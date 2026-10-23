"""块内文本归一化与句子拆分（查重比对前置）。

归一化仅在单个内容块内进行：产出 (归一化文本, 归一化→原始偏移映射)，
比对坐标最终经映射换算回块原始文本偏移，前端零换算圈选。
"""
from __future__ import annotations

import re
import unicodedata

# NFKC 覆盖不到的中文标点 → 半角（translate 一对一，不破坏偏移映射；None=删除）
_PUNCT_MAP = {
    ord("。"): ".",
    ord("、"): ",",
    ord("《"): "<",
    ord("》"): ">",
    ord("【"): "[",
    ord("】"): "]",
    ord("“"): '"',
    ord("”"): '"',
    ord("‘"): "'",
    ord("’"): "'",
    ord("—"): "-",
    ord("－"): "-",
    ord("～"): "~",
    ord("…"): None,
}
# 零宽/软连字符/控制字符：直接删除（不占归一化位置）
_DROP_CHARS = frozenset("\u200b\u200c\u200d\u2060\ufeff\u00ad")

# 句子结束标点（归一化后）；"." 需数字守卫（小数/编号不切分）
_SENT_ENDERS = frozenset("。！？；!?;.")

# 纯编号/纯标点句子：无实义内容，剔除（作用于归一化文本，分隔符为半角形态）
_PURE_NUM_RE = re.compile(r"^[（(【\[]?\s*[\d一二三四五六七八九十百①-⑩]{1,6}\s*[)）】\],，.．]?\s*$")
_HAS_CONTENT_RE = re.compile(r"[\u4e00-\u9fffA-Za-z0-9]")

# 句首编号前缀（归一化形态）：多级数字编号可直接剥离；单级序号必须带分隔符（保护 "3年以上"）
_NUM_PREFIX_RE = re.compile(
    r"^\s*[（(【\[]?\s*"
    r"(?:\d+(?:[.．]\d+)+(?!\.?\d)"
    r"|[\d一二三四五六七八九十百①-⑩]{1,8}\s*[)）】\],，.．:：]"
    r")\s*"
)

# 进索引/比对的最短句子（归一化字符数）；更短者视为噪声
MIN_SENT_CHARS = 4


def normalize(text: str) -> tuple[str, list[int]]:
    """块原始文本 → (归一化文本, norm_to_raw)。

    norm_to_raw[i] = 归一化文本第 i 个字符在原始文本中的下标
    （NFKC 一变多时多个归一化字符指向同一原始下标）。
    """
    out: list[str] = []
    mapping: list[int] = []
    prev_space = True  # 抑制前导空白
    for i, ch in enumerate(text):
        if ch.isspace():  # 先于控制字符判断：\t\n 等折叠为单空格而非删除
            if not prev_space:
                out.append(" ")
                mapping.append(i)
                prev_space = True
            continue
        if ch in _DROP_CHARS or ord(ch) < 32:  # \t\n\r 已在上面的 isspace 分支处理
            continue
        prev_space = False
        for c in unicodedata.normalize("NFKC", ch).lower().translate(_PUNCT_MAP):
            out.append(c)
            mapping.append(i)
    while out and out[-1] == " ":  # 尾部空白 strip
        out.pop()
        mapping.pop()
    return "".join(out), mapping


def split_sentences(norm: str) -> list[tuple[int, int]]:
    """归一化文本 → 句子区间 [(start, end)]（end 不含结尾标点，左闭右开）。

    边界处理："." 仅在非数字邻接时切分（小数 1.5 / 编号 2.1 不拆）；
    纯编号、纯标点、无实义内容的区间剔除。
    """
    spans: list[tuple[int, int]] = []
    start = 0
    n = len(norm)
    for i, ch in enumerate(norm):
        if ch not in _SENT_ENDERS:
            continue
        if ch == "." and i > 0 and i + 1 < n and norm[i - 1].isdigit() and norm[i + 1].isdigit():
            continue  # 小数/多级编号中的点（两侧均为数字）不切分
        spans.append((start, i))
        start = i + 1
    if start < n:
        spans.append((start, n))

    out: list[tuple[int, int]] = []
    for s, e in spans:
        while s < e and norm[s] == " ":
            s += 1
        while e > s and norm[e - 1] in _SENT_ENDERS | {" "}:
            e -= 1
        m = _NUM_PREFIX_RE.match(norm, s, e)  # 剥离句首编号前缀（如 "2.1" / "（一）"）
        if m and m.end() < e:
            s = m.end()
        if e - s < MIN_SENT_CHARS:
            continue
        seg = norm[s:e]
        if _PURE_NUM_RE.match(seg) or not _HAS_CONTENT_RE.search(seg):
            continue
        out.append((s, e))
    return out
