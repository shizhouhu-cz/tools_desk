import { useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { ArrowLeftRight, FileDiff } from "lucide-react";
import { toast } from "sonner";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ToolShell } from "@/components/layout/ToolShell";
import { ProgressPanel, RunBar, fileName } from "@/components/common";
import { docxCompare, normalizeError, pickFile } from "@/lib/tauri";
import { formatDuration, useTask } from "@/lib/task";
import type {
  CompareCellNode,
  CompareFragment,
  CompareNode,
  CompareParaNode,
  CompareResult,
  CompareStats,
} from "@/lib/types";

const CONF_LABEL: Record<string, string> = {
  high: "高置信",
  mid: "中置信",
  low: "低置信",
};

const MARK_CLS: Record<string, string> = {
  high: "bg-red-500/30 rounded-sm",
  mid: "bg-amber-500/30 rounded-sm",
  low: "bg-sky-500/25 rounded-sm",
};

const CONF_BADGE_CLS: Record<string, string> = {
  high: "bg-red-500/15 text-red-600 dark:text-red-400",
  mid: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  low: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
};

/** 块内高亮标记（原始文本偏移） */
interface Mark {
  start: number;
  end: number;
  confidence: string;
}

/** 按 (block, start) 合并重叠区间后切片渲染 <mark>。 */
function renderMarked(text: string, marks: Mark[], keyPrefix: string): ReactNode {
  if (marks.length === 0) return text;
  const merged: Mark[] = [];
  for (const m of [...marks].sort((x, y) => x.start - y.start)) {
    const last = merged[merged.length - 1];
    if (last && m.start <= last.end) {
      last.end = Math.max(last.end, m.end);
    } else {
      merged.push({ ...m });
    }
  }
  const out: ReactNode[] = [];
  let pos = 0;
  merged.forEach((m, i) => {
    if (m.start > pos) out.push(text.slice(pos, m.start));
    out.push(
      <mark key={`${keyPrefix}-${i}`} className={`${MARK_CLS[m.confidence] ?? MARK_CLS.low} text-inherit`}>
        {text.slice(m.start, m.end)}
      </mark>
    );
    pos = m.end;
  });
  if (pos < text.length) out.push(text.slice(pos));
  return out;
}

/** 片段 → 按文档侧分组的 {block: marks} 映射。 */
function buildMarks(fragments: CompareFragment[]): Record<"a" | "b", Map<number, Mark[]>> {
  const maps: Record<"a" | "b", Map<number, Mark[]>> = {
    a: new Map(),
    b: new Map(),
  };
  for (const f of fragments) {
    for (const side of ["a", "b"] as const) {
      const loc = f[side];
      const list = maps[side].get(loc.block) ?? [];
      list.push({ start: loc.start, end: loc.end, confidence: f.confidence });
      maps[side].set(loc.block, list);
    }
  }
  return maps;
}

function ParaBlock({ node, marks }: { node: CompareParaNode; marks: Mark[] }) {
  const style: CSSProperties = {};
  const align = node.align;
  if (align === "left" || align === "center" || align === "right" || align === "justify") {
    style.textAlign = align;
  }
  if (node.font_halfpt) style.fontSize = `${node.font_halfpt / 2}pt`;
  const text = (node.number_prefix ? `${node.number_prefix} ` : "") + node.text;
  if (node.style === "heading") {
    style.fontWeight = 700;
    style.marginTop = node.outline === 1 ? "1.25em" : "0.9em";
    style.marginBottom = "0.4em";
    return (
      <p data-block-id={node.block} style={style}>
        {renderMarked(text, marks, `p${node.block}`)}
      </p>
    );
  }
  if (node.style === "list") {
    style.marginLeft = "1.5em";
  }
  return (
    <p data-block-id={node.block} style={style} className="leading-7">
      {renderMarked(text, marks, `p${node.block}`)}
    </p>
  );
}

function TableBlock({
  node,
  marks,
}: {
  node: Extract<CompareNode, { kind: "table" }>;
  marks: (block: number) => Mark[];
}) {
  const cellMap = new Map<string, CompareCellNode>();
  const covered = new Set<string>();
  for (const c of node.cells) {
    cellMap.set(`${c.row},${c.col}`, c);
    for (let r = c.row; r < c.row + c.rowSpan; r++) {
      for (let cc = c.col; cc < c.col + c.colSpan; cc++) {
        if (r !== c.row || cc !== c.col) covered.add(`${r},${cc}`);
      }
    }
  }
  const rows = Array.from({ length: node.rows }, (_, r) => r);
  const cols = Array.from({ length: node.cols }, (_, c) => c);
  return (
    <table data-block-id={node.block} className="my-3 w-full border-collapse text-xs">
      <tbody>
        {rows.map((r) => (
          <tr key={r}>
            {cols.map((c) => {
              if (!cellMap.has(`${r},${c}`)) {
                if (covered.has(`${r},${c}`)) return null; // 被 rowSpan/colSpan 覆盖
                return <td key={c} className="border px-2 py-1" />;
              }
              const cell = cellMap.get(`${r},${c}`)!;
              return (
                <td
                  key={c}
                  data-block-id={cell.block}
                  rowSpan={cell.rowSpan > 1 ? cell.rowSpan : undefined}
                  colSpan={cell.colSpan > 1 ? cell.colSpan : undefined}
                  className={`border px-2 py-1 align-top leading-6 ${cell.isHeader ? "bg-muted/60 font-medium" : ""
                    }`}
                >
                  {renderMarked(cell.text, marks(cell.block), `c${cell.block}`)}
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function DocPane({
  name,
  tree,
  marks,
  side,
}: {
  name: string;
  tree: CompareNode[];
  marks: Map<number, Mark[]>;
  side: "a" | "b";
}) {
  return (
    <div className="space-y-1">
      <div className="sticky top-0 z-10 bg-card px-1 pb-1 text-xs font-medium text-muted-foreground">
        {side === "a" ? "基准文档" : "待查文档"}：{name}
      </div>
      <div className="space-y-1.5 text-sm">
        {tree.map((node) =>
          node.kind === "paragraph" ? (
            <ParaBlock key={node.block} node={node} marks={marks.get(node.block) ?? []} />
          ) : (
            <TableBlock key={node.block} node={node} marks={(b) => marks.get(b) ?? []} />
          )
        )}
      </div>
    </div>
  );
}

const pct = (v: number) => `${(v * 100).toFixed(1)}%`;

export default function DocxCompare() {
  const [pathA, setPathA] = useState("");
  const [pathB, setPathB] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CompareResult | null>(null);
  const [activeFrag, setActiveFrag] = useState<number | null>(null);
  const task = useTask("docx-compare");

  const aPaneRef = useRef<HTMLDivElement>(null);
  const bPaneRef = useRef<HTMLDivElement>(null);

  const marks = useMemo(
    () => buildMarks(result?.fragments ?? []),
    [result]
  );

  const pick = async (side: "a" | "b") => {
    const p = await pickFile(side === "a" ? "选择基准文档" : "选择待查文档", ["docx"]);
    if (!p) return;
    if (side === "a") setPathA(p);
    else setPathB(p);
  };

  const swap = () => {
    setPathA(pathB);
    setPathB(pathA);
  };

  const run = async () => {
    if (!pathA || !pathB) {
      toast.warning("请先选择基准文档与待查文档");
      return;
    }
    if (pathA === pathB) {
      toast.warning("基准文档与待查文档不能是同一个文件");
      return;
    }
    setResult(null);
    setActiveFrag(null);
    setRunning(true);
    try {
      const r = await docxCompare(pathA, pathB);
      setResult(r);
      toast.success(
        `比对完成：待查文档重复率 ${pct(r.stats.rate_b)}，检出片段 ${r.stats.fragment_count} 个`
      );
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  const scrollToBlock = (root: HTMLElement | null, block: number) => {
    const el = root?.querySelector(`[data-block-id="${block}"]`);
    if (!el) return;
    el.scrollIntoView({ block: "center", behavior: "instant" });
    el.classList.remove("cmp-flash");
    // 强制重启动画
    void (el as HTMLElement).offsetWidth;
    el.classList.add("cmp-flash");
    window.setTimeout(() => el.classList.remove("cmp-flash"), 1400);
  };

  const jump = (i: number) => {
    const f = result?.fragments[i];
    if (!f) return;
    setActiveFrag(i);
    scrollToBlock(aPaneRef.current, f.a.block);
    scrollToBlock(bPaneRef.current, f.b.block);
  };

  const stats: CompareStats | null = result?.stats ?? null;

  return (
    <ToolShell
      title="DOCX 标书比对"
      description="对两份 DOCX 做文本查重：倒排召回 + 分层精比定位重复片段，双栏同源渲染并高亮，输出重复率与分章节统计。"
    >
      <style>{`
        @keyframes cmp-flash { 0%, 55% { background-color: color-mix(in srgb, var(--primary) 22%, transparent); } 100% { background-color: transparent; } }
        .cmp-flash { animation: cmp-flash 1.4s ease-out; }
      `}</style>

      <div className="mx-auto max-w-6xl space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">输入文档</CardTitle>
            <CardDescription>
              A 为基准文档，B 为待查文档（重复率以 B 视角统计）；顺序不影响片段检出，可随时互换。
            </CardDescription>
          </CardHeader>
          <CardContent className="grid items-center gap-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]">
            <FileSlot label="基准文档 A" path={pathA} onPick={() => pick("a")} />
            <Button
              variant="ghost"
              size="icon"
              onClick={swap}
              disabled={running}
              title="交换 A / B"
            >
              <ArrowLeftRight className="size-4" />
            </Button>
            <FileSlot label="待查文档 B" path={pathB} onPick={() => pick("b")} />
          </CardContent>
        </Card>

        <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_360px]">
          <RunBar
            running={running}
            onRun={run}
            label="开始比对"
            runningLabel="比对中…"
            hint="大文档比对通常在数秒内完成；比对基于清洗归一化后的文本，全半角与空白差异不影响检出。"
          />
          <ProgressPanel task={task} onCancel={task.cancel} />
        </div>

        {stats && result && (
          <>
            <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <StatTile
                label="B 待查重复率"
                value={pct(stats.rate_b)}
                sub={`A 基准 ${pct(stats.rate_a)}`}
                emphasize
              />
              <StatTile
                label="检出片段"
                value={String(stats.fragment_count)}
                sub={`最长 ${stats.longest} 字`}
              />
              <StatTile
                label="高 / 中 / 低置信"
                value={`${stats.by_confidence.high?.fragments ?? 0} / ${stats.by_confidence.mid?.fragments ?? 0
                  } / ${stats.by_confidence.low?.fragments ?? 0}`}
                sub={`高置信覆盖 B ${stats.by_confidence.high?.chars_b ?? 0} 字`}
              />
              <StatTile
                label="有效字符"
                value={`${stats.total_chars_b.toLocaleString()}`}
                sub={`A ${stats.total_chars_a.toLocaleString()} · 耗时 ${formatDuration(
                  stats.elapsed_seconds * 1000
                )}`}
              />
            </div>

            <Card>
              <CardHeader>
                <CardTitle className="text-base">双栏比对</CardTitle>
                <CardDescription>
                  点击片段列表可定位到两侧对应位置；高亮颜色区分置信度（红=高 / 橙=中 / 蓝=低）。
                </CardDescription>
              </CardHeader>
              <CardContent className="grid gap-4 lg:grid-cols-2">
                <div
                  ref={aPaneRef}
                  className="max-h-[65vh] overflow-y-auto rounded-md border bg-card p-3"
                >
                  <DocPane
                    name={result.docs.a.name}
                    tree={result.docs.a.tree}
                    marks={marks.a}
                    side="a"
                  />
                </div>
                <div
                  ref={bPaneRef}
                  className="max-h-[65vh] overflow-y-auto rounded-md border bg-card p-3"
                >
                  <DocPane
                    name={result.docs.b.name}
                    tree={result.docs.b.tree}
                    marks={marks.b}
                    side="b"
                  />
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle className="text-base">
                  重复片段
                  <span className="ml-2 text-sm font-normal text-muted-foreground">
                    共 {result.fragments.length} 个
                  </span>
                </CardTitle>
              </CardHeader>
              <CardContent className="max-h-96 space-y-1.5 overflow-y-auto">
                {result.fragments.length === 0 && (
                  <p className="py-6 text-center text-sm text-muted-foreground">
                    未检出重复片段。
                  </p>
                )}
                {result.fragments.map((f, i) => (
                  <button
                    key={`${f.b.block}-${f.b.start}-${i}`}
                    onClick={() => jump(i)}
                    className={`flex w-full items-center gap-2 rounded-md border px-3 py-2 text-left text-sm transition-colors hover:bg-accent/60 ${activeFrag === i ? "border-primary/60 bg-accent/40" : ""
                      }`}
                  >
                    <Badge className={`shrink-0 ${CONF_BADGE_CLS[f.confidence]}`}>
                      {CONF_LABEL[f.confidence]}
                    </Badge>
                    <Badge variant="outline" className="shrink-0">
                      {f.match_type}
                    </Badge>
                    <span className="min-w-0 flex-1 truncate" title={f.content}>
                      {f.content}
                    </span>
                    <span className="shrink-0 whitespace-nowrap text-xs text-muted-foreground tabular-nums">
                      连续 {f.chain} 句 · {f.length} 字
                    </span>
                  </button>
                ))}
              </CardContent>
            </Card>

            {stats.by_chapter.length > 0 && (
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">分章节重复统计（待查文档 B）</CardTitle>
                  <CardDescription>按覆盖字符数降序，重复率 = 覆盖字符 / 章节有效字符。</CardDescription>
                </CardHeader>
                <CardContent className="max-h-72 space-y-1.5 overflow-y-auto">
                  {stats.by_chapter.map((ch) => (
                    <div key={ch.chapter} className="flex items-center gap-3 text-sm">
                      <span className="min-w-0 flex-1 truncate" title={ch.chapter}>
                        {ch.chapter}
                      </span>
                      <div className="h-2 w-32 shrink-0 overflow-hidden rounded-full bg-muted">
                        <div
                          className="h-full rounded-full bg-primary/70"
                          style={{ width: `${Math.min(ch.rate * 100, 100)}%` }}
                        />
                      </div>
                      <span className="w-14 shrink-0 text-right tabular-nums">
                        {pct(ch.rate)}
                      </span>
                      <span className="w-24 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                        {ch.covered} / {ch.total}
                      </span>
                    </div>
                  ))}
                </CardContent>
              </Card>
            )}
          </>
        )}

        {!result && !running && (
          <Card className="border-dashed">
            <CardContent className="flex flex-col items-center gap-2 py-10 text-muted-foreground">
              <FileDiff className="size-8 opacity-50" />
              <p className="text-sm">选择两份 DOCX 后开始比对，结果将在此展示。</p>
            </CardContent>
          </Card>
        )}
      </div>
    </ToolShell>
  );
}

function FileSlot({ label, path, onPick }: { label: string; path: string; onPick: () => void }) {
  return (
    <button
      onClick={onPick}
      className="flex h-16 w-full flex-col justify-center gap-0.5 rounded-md border px-3 text-left transition-colors hover:bg-accent/60"
      title={path || label}
    >
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className={`truncate text-sm ${path ? "" : "text-muted-foreground"}`}>
        {path ? fileName(path) : "点击选择 DOCX 文件"}
      </span>
    </button>
  );
}

function StatTile({
  label,
  value,
  sub,
  emphasize,
}: {
  label: string;
  value: string;
  sub?: string;
  emphasize?: boolean;
}) {
  return (
    <Card className={emphasize ? "border-primary/40" : ""}>
      <CardContent className="space-y-1 py-4">
        <div className="text-xs text-muted-foreground">{label}</div>
        <div
          className={`text-2xl font-semibold tabular-nums ${emphasize ? "text-primary" : ""
            }`}
        >
          {value}
        </div>
        {sub && <div className="text-xs text-muted-foreground">{sub}</div>}
      </CardContent>
    </Card>
  );
}
