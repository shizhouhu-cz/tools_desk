"""查重比对引擎：召回 → 三层比对 → 字符级定位 → 片段与统计。

漏斗式分层过滤（design 3.3-3.6，Python 实现）：
  3-gram 词级倒排召回 Top-N → Jaccard 初筛 → Levenshtein 精算 → 连续匹配链 DP
  → difflib 公共子串定位 → (block, 块内原始偏移) 片段集合。
无全局偏移映射：归一化仅在块内，片段坐标已换算回块原始文本偏移。
"""
from __future__ import annotations

import re
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from difflib import SequenceMatcher
from pathlib import Path
from typing import Callable

from doc_analyze.compare import normalize as norm_mod
from doc_analyze.compare.blocks import build_block_tree

ProgressFn = Callable[[str, int, int, str | None], None]

_HAS_CONTENT = re.compile(r"[\u4e00-\u9fffA-Za-z0-9]")

# ---- 可调参数（v1 内置默认；阶段三并入设置页）----
RECALL_MIN = 0.3          # 倒排召回阈值：命中碎片 / 句子碎片数
RECALL_TOP_N = 5          # 每个待查句子保留的候选数
JACCARD_MIN = 0.25        # 第一层：词集 Jaccard 初筛阈值
LEV_MIN = 0.6             # 第二层：编辑距离相似度阈值（1 - dist/max_len）
MIN_FRAG_CHARS = 8        # 字符级片段最小长度（归一化字符）
MERGE_GAP = 3             # 相邻片段间隔 ≤ 此值则合并
CHAIN_HIGH = 3            # 连续匹配 ≥ 3 句 → 高置信度
CHAIN_MID = 2             # 连续匹配 = 2 句 → 中置信度；孤立单句 → 低

# ---- 停用词（通用中文 + 常用英文；仅用于相似度计算，不影响定位）----
STOPWORDS = frozenset(
    "的 了 在 是 我 你 他 她 它 我们 你们 他们 她们 它们 这 那 这个 那个 这些 那些 "
    "和 与 及 或 等 就 都 而 之 以 于 对 为 由 从 被 把 让 使 按照 根据 有关 相关 "
    "进行 通过 对于 由于 因此 但是 然而 如果 那么 以及 或者 并且 同时 此外 另外 "
    "其 该 各 每 个 项 项下 中 上 内 外 前 后 时 将 已 未 可 应 需 须 可以 必须 应当 "
    "如下 以下 以上 所 及其 其中 之间 之后 之前 不得 应当 具有 具备 包括 包含 "
    "the a an of to in and or is are was were be been being for with on at by from as "
    "that this these those it its not no but if then than so such"
).union("\u3000 \t".split())


# ---- 分词：jieba 优先，缺包时回退字符级（CJK 单字 + 拉丁词 + 数字）----
try:
    import jieba

    jieba.setLogLevel(60)  # 静默构建日志

    def tokenize(text: str) -> list[str]:
        return jieba.lcut(text)

except ImportError:  # pragma: no cover - 兜底路径
    import re as _re

    _TOKEN_RE = _re.compile(r"[A-Za-z]+|\d+(?:\.\d+)?|[\u4e00-\u9fff]")

    def tokenize(text: str) -> list[str]:
        return _TOKEN_RE.findall(text)


# ---- 编辑距离：rapidfuzz 优先，缺包时纯 Python DP（仅候选对精算，量小）----
try:
    from rapidfuzz.distance import Levenshtein as _Lv

    def _lev_dist(a: str, b: str) -> int:
        return _Lv.distance(a, b)

except ImportError:  # pragma: no cover - 兜底路径
    def _lev_dist(a: str, b: str) -> int:
        if a == b:
            return 0
        if not a:
            return len(b)
        if not b:
            return len(a)
        prev = list(range(len(b) + 1))
        for i, ca in enumerate(a, 1):
            cur = [i]
            for j, cb in enumerate(b, 1):
                cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
            prev = cur
        return prev[-1]


@dataclass
class Sent:
    """句子单元：比对的最小执行单位。"""

    doc: int              # 0=A 基准 / 1=B 待查
    seq: int              # 文档顺序全局序号（连续匹配链 DP 用）
    block: int            # 所属内容块
    start: int            # 块归一化文本内起始偏移
    end: int              # 块归一化文本内结束偏移（不含）
    raw_start: int        # 块原始文本内起始偏移（已换算）
    raw_end: int
    norm: str
    words: frozenset[str] = field(default_factory=frozenset)
    grams: list[int] = field(default_factory=list)


def _content_words(norm: str) -> list[str]:
    """内容词：去停用词、去纯标点 token（jieba 会切出标点 token）。"""
    return [
        w
        for w in tokenize(norm)
        if w not in STOPWORDS and not w.isspace() and _HAS_CONTENT.search(w)
    ]


def _grams(words: list[str]) -> list[int]:
    """词级 3-gram 哈希；不足 3 词时整句词序作单一碎片。"""
    if len(words) >= 3:
        return [hash((words[i], words[i + 1], words[i + 2])) for i in range(len(words) - 2)]
    return [hash(tuple(words))] if words else []


def _build_sents(doc: int, tree_doc: dict) -> list[Sent]:
    """块树 → 句子单元列表（文档顺序，坐标为块内归一化偏移）。"""
    sents: list[Sent] = []
    blocks = tree_doc["blocks"]
    for block_id, meta in blocks.items():
        raw = meta["text"]
        norm_text, mapping = norm_mod.normalize(raw)
        if not norm_text:
            continue
        for s, e in norm_mod.split_sentences(norm_text):
            raw_s, raw_e = mapping[s], mapping[e - 1] + 1
            words = _content_words(norm_text[s:e])
            sent = Sent(
                doc=doc,
                seq=len(sents),
                block=block_id,
                start=s,
                end=e,
                raw_start=raw_s,
                raw_end=raw_e,
                norm=norm_text[s:e],
                words=frozenset(words),
                grams=_grams(words),
            )
            sents.append(sent)
    return sents


def _recall(a_sents: list[Sent], b_sents: list[Sent], progress: ProgressFn | None) -> set[tuple[int, int]]:
    """3-gram 倒排召回：(A 句 seq, B 句 seq) 候选对集合。"""
    index: dict[int, list[int]] = defaultdict(list)
    for s in a_sents:
        for g in s.grams:
            index[g].append(s.seq)

    pairs: set[tuple[int, int]] = set()
    total = len(b_sents)
    for s in b_sents:
        if progress and s.seq % 200 == 0:
            progress("recall", s.seq, total, None)
        if not s.grams:
            continue
        hits: Counter[int] = Counter()
        for g in s.grams:
            for a_seq in index.get(g, ()):
                hits[a_seq] += 1
        if not hits:
            continue
        top = hits.most_common(RECALL_TOP_N)
        for a_seq, cnt in top:
            if cnt / len(s.grams) >= RECALL_MIN:
                pairs.add((a_seq, s.seq))
    return pairs


def _chain_lengths(pairs: list[tuple[int, int]]) -> dict[tuple[int, int], int]:
    """连续匹配链 DP：(ai, bj) 相似且 (ai-1, bj-1) 相似 → 链长 +1。"""
    best: dict[tuple[int, int], int] = {}
    for ia, ib in sorted(pairs):
        best[(ia, ib)] = best.get((ia - 1, ib - 1), 0) + 1
    return best


def _merge_blocks(blocks: list[tuple[int, int, int]]) -> list[tuple[int, int, int]]:
    """(a_start, b_start, length) 匹配块：a/b 两维间隔均 ≤ MERGE_GAP 时合并。"""
    if not blocks:
        return []
    merged = [list(blocks[0])]
    for a_s, b_s, n in blocks[1:]:
        last = merged[-1]
        a_end = last[0] + last[2]
        b_end = last[1] + last[2]
        if a_s - a_end <= MERGE_GAP and b_s - b_end <= MERGE_GAP:
            last[2] = max(a_s + n, a_end) - last[0]
            # 合并后区间终点取两维各自最远端（高亮含中间缝隙，可接受）
        else:
            merged.append([a_s, b_s, n])
    return [tuple(m) for m in merged]


def _confidence(chain_len: int, lev_sim: float, norm_len: int) -> str:
    """置信度分级：连续匹配链优先；整句高相似长文本同样视为强信号。

    孤立单句低相似 → low（套话巧合）；连续 ≥3 句或整句一致长文本 → high。
    """
    if chain_len >= CHAIN_HIGH or (lev_sim >= 0.99 and norm_len >= 20):
        return "high"
    if chain_len >= CHAIN_MID or (lev_sim >= 0.9 and norm_len >= 12):
        return "mid"
    return "low"


def _locate_fragments(
    a_sents: list[Sent],
    b_sents: list[Sent],
    pairs: dict[tuple[int, int], float],
    chain: dict[tuple[int, int], int],
    tree_a: dict,
    tree_b: dict,
) -> list[dict]:
    """确认对 → 字符级公共子串定位 → 片段列表（块原始文本偏移）。"""
    maps: list[dict[int, tuple[str, list[int]]]] = [{}, {}]
    for d, tree in ((0, tree_a), (1, tree_b)):
        for block_id, meta in tree["blocks"].items():
            norm_text, mapping = norm_mod.normalize(meta["text"])
            maps[d][block_id] = (meta["text"], mapping)

    fragments: list[dict] = []
    for (ia, ib), lev_sim in pairs.items():
        sa, sb = a_sents[ia], b_sents[ib]
        confidence = _confidence(chain[(ia, ib)], lev_sim, len(sa.norm))
        sm = SequenceMatcher(None, sa.norm, sb.norm, autojunk=False)
        raw_blocks = [
            (blk.a, blk.b, blk.size) for blk in sm.get_matching_blocks() if blk.size > 0
        ]
        for a_s, b_s, n in _merge_blocks(raw_blocks):
            if n < MIN_FRAG_CHARS:
                continue
            _, map_a = maps[0][sa.block]
            _, map_b = maps[1][sb.block]
            a_raw_s, a_raw_e = map_a[a_s], map_a[a_s + n - 1] + 1
            b_raw_s, b_raw_e = map_b[b_s], map_b[b_s + n - 1] + 1
            meta_a = tree_a["blocks"][sa.block]
            fragments.append(
                {
                    "a": {"block": sa.block, "start": a_raw_s, "end": a_raw_e},
                    "b": {"block": sb.block, "start": b_raw_s, "end": b_raw_e},
                    "length": n,
                    "confidence": confidence,
                    "chain": chain[(ia, ib)],
                    "content": meta_a["text"][a_raw_s:a_raw_e],
                    "match_type": meta_a["match_type"],
                }
            )

    fragments.sort(key=lambda f: (f["b"]["block"], f["b"]["start"]))
    return fragments


def _merge_intervals(intervals: list[tuple[int, int]]) -> list[tuple[int, int]]:
    if not intervals:
        return []
    intervals = sorted(intervals)
    out = [intervals[0]]
    for s, e in intervals[1:]:
        if s <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], e))
        else:
            out.append((s, e))
    return out


def _coverage(fragments: list[dict], side: str) -> dict[int, int]:
    """按块统计覆盖的归一化字符数（同块区间并集，避免重复计数）。"""
    per_block: dict[int, list[tuple[int, int]]] = defaultdict(list)
    for f in fragments:
        loc = f[side]
        per_block[loc["block"]].append((loc["start"], loc["end"]))
    return {b: sum(e - s for s, e in _merge_intervals(iv)) for b, iv in per_block.items()}


def _block_norm_len(tree: dict) -> dict[int, int]:
    """各块的有效（归一化）字符数。"""
    out = {}
    for block_id, meta in tree["blocks"].items():
        norm_text, _ = norm_mod.normalize(meta["text"])
        out[block_id] = len(norm_text)
    return out


def _compute_stats(
    tree_a: dict,
    tree_b: dict,
    fragments: list[dict],
    elapsed: float,
) -> dict:
    """重复率与多维统计。覆盖率按块内区间并集去重。"""
    lens_a, lens_b = _block_norm_len(tree_a), _block_norm_len(tree_b)
    total_a, total_b = sum(lens_a.values()), sum(lens_b.values())
    cov_a = sum(_coverage(fragments, "a").values())
    cov_b = sum(_coverage(fragments, "b").values())

    by_conf: dict[str, dict] = {}
    for level in ("high", "mid", "low"):
        subset = [f for f in fragments if f["confidence"] == level]
        by_conf[level] = {
            "fragments": len(subset),
            "chars_a": sum(_coverage(subset, "a").values()),
            "chars_b": sum(_coverage(subset, "b").values()),
        }

    # 分章节统计（B 待查文档视角）
    cov_b_by_block = _coverage(fragments, "b")
    by_chapter: dict[str, dict] = {}
    for block_id, meta in tree_b["blocks"].items():
        ch = meta["chapter"] or "（未分章）"
        agg = by_chapter.setdefault(ch, {"total": 0, "covered": 0})
        agg["total"] += lens_b.get(block_id, 0)
        agg["covered"] += cov_b_by_block.get(block_id, 0)
    chapters = [
        {
            "chapter": ch,
            "total": v["total"],
            "covered": v["covered"],
            "rate": round(v["covered"] / v["total"], 4) if v["total"] else 0.0,
        }
        for ch, v in sorted(by_chapter.items(), key=lambda kv: -kv[1]["covered"])
    ]

    return {
        "total_chars_a": total_a,
        "total_chars_b": total_b,
        "rate_a": round(cov_a / total_a, 4) if total_a else 0.0,
        "rate_b": round(cov_b / total_b, 4) if total_b else 0.0,
        "fragment_count": len(fragments),
        "longest": max((f["length"] for f in fragments), default=0),
        "by_confidence": by_conf,
        "by_chapter": chapters,
        "elapsed_seconds": round(elapsed, 2),
    }


def run_compare(
    path_a: str | Path, path_b: str | Path, progress: ProgressFn | None = None
) -> dict:
    """双文档查重主流程。返回 {"docs": {...}, "fragments": [...], "stats": {...}}。"""
    started = time.perf_counter()

    def report(stage: str, current: int, total: int, message: str | None = None) -> None:
        if progress:
            progress(stage, current, total, message)

    report("parse", 1, 2, "解析基准文档")
    tree_a = build_block_tree(path_a, progress)
    report("parse", 2, 2, "解析待查文档")
    tree_b = build_block_tree(path_b, progress)

    report("index", 0, 0, "归一化与句子拆分")
    a_sents = _build_sents(0, tree_a)
    b_sents = _build_sents(1, tree_b)
    report("index", 0, 0, f"A {len(a_sents)} 句 / B {len(b_sents)} 句，构建倒排索引")

    pairs = _recall(a_sents, b_sents, progress)
    report("match", 0, 0, f"召回候选 {len(pairs)} 对，精细比对中")

    # 三层递进：Jaccard 初筛 → Levenshtein 精算 → 连续匹配链
    confirmed: dict[tuple[int, int], float] = {}  # (A seq, B seq) → lev 相似度
    for ia, ib in pairs:
        sa, sb = a_sents[ia], b_sents[ib]
        if not sa.words or not sb.words:
            continue
        inter = len(sa.words & sb.words)
        union = len(sa.words | sb.words)
        if union == 0 or inter / union < JACCARD_MIN:
            continue
        max_len = max(len(sa.norm), len(sb.norm))
        lev = 1 - _lev_dist(sa.norm, sb.norm) / max_len if max_len else 0.0
        if lev < LEV_MIN:
            continue
        confirmed[(ia, ib)] = lev

    chain = _chain_lengths(confirmed)
    report("match", len(confirmed), max(len(confirmed), 1), f"确认相似句对 {len(confirmed)} 对")

    fragments = _locate_fragments(a_sents, b_sents, confirmed, chain, tree_a, tree_b)
    stats = _compute_stats(tree_a, tree_b, fragments, time.perf_counter() - started)

    return {
        "docs": {
            "a": {"name": Path(path_a).name, "tree": tree_a["tree"]},
            "b": {"name": Path(path_b).name, "tree": tree_b["tree"]},
        },
        "fragments": fragments,
        "stats": stats,
    }
