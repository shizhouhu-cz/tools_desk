import { useEffect, useMemo, useRef, useState } from "react";
import { Loader2, Search, X } from "lucide-react";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { fileName } from "@/components/common";
import { docxChapterContent, docxTree, normalizeError } from "@/lib/tauri";
import type { ChapterNode } from "@/lib/types";

export interface TreeTarget {
  docId: number;
  docName: string;
}

/** 前端关键字过滤：命中节点保留其全部子树，未命中但有命中后代则保留过滤后的后代。 */
function filterTree(nodes: ChapterNode[], kwLower: string): ChapterNode[] {
  const out: ChapterNode[] = [];
  for (const n of nodes) {
    const children = filterTree(n.children, kwLower);
    const selfHit = `${n.number_prefix} ${n.title}`.toLowerCase().includes(kwLower);
    if (selfHit) {
      out.push(n);
    } else if (children.length > 0) {
      out.push({ ...n, children });
    }
  }
  return out;
}

function countNodes(nodes: ChapterNode[]): number {
  return nodes.reduce((acc, n) => acc + 1 + countNodes(n.children), 0);
}

function TreeRows({
  nodes,
  depth,
  selectedId,
  onSelect,
}: {
  nodes: ChapterNode[];
  depth: number;
  selectedId: number | null;
  onSelect: (n: ChapterNode) => void;
}) {
  return (
    <>
      {nodes.map((n) => (
        <div key={n.id}>
          <div
            className={cn(
              "flex cursor-pointer items-baseline gap-2 rounded-md py-1 pr-2 text-sm hover:bg-muted/60",
              selectedId === n.id && "bg-muted"
            )}
            style={{ paddingLeft: depth * 16 + 8 }}
            onClick={() => onSelect(n)}
          >
            {n.number_prefix && (
              <span className="shrink-0 tabular-nums text-muted-foreground">
                {n.number_prefix}
              </span>
            )}
            <span
              className={cn(
                "min-w-0 truncate",
                n.needs_review && "text-amber-600 dark:text-amber-400"
              )}
              title={n.title}
            >
              {n.title}
              {n.needs_review && " *"}
            </span>
            {n.content_chars != null && (
              <span className="ml-auto shrink-0 tabular-nums text-xs text-muted-foreground">
                {n.content_chars} 字符
              </span>
            )}
          </div>
          {n.children.length > 0 && (
            <TreeRows
              nodes={n.children}
              depth={depth + 1}
              selectedId={selectedId}
              onSelect={onSelect}
            />
          )}
        </div>
      ))}
    </>
  );
}

/** 章节树弹窗：打开时懒加载 docx_tree，前端关键字过滤；点击节点懒加载正文内容。 */
export default function ChapterTreeDialog({
  target,
  onClose,
}: {
  target: TreeTarget;
  onClose: () => void;
}) {
  const [tree, setTree] = useState<ChapterNode[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<ChapterNode | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [contentLoading, setContentLoading] = useState(false);
  const [contentError, setContentError] = useState<string | null>(null);
  // 竞态保护：只接受最近一次请求的结果。
  const reqIdRef = useRef<number | null>(null);

  useEffect(() => {
    let alive = true;
    setTree(null);
    setError(null);
    setFilter("");
    setSelected(null);
    setContent(null);
    docxTree(target.docId)
      .then((t) => {
        if (alive) setTree(t);
      })
      .catch((e) => {
        if (alive) setError(normalizeError(e).message);
      });
    return () => {
      alive = false;
    };
  }, [target.docId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const selectNode = (n: ChapterNode) => {
    if (selected?.id === n.id) return; // 重复点击同一节点不重新加载
    reqIdRef.current = n.id;
    setSelected(n);
    setContent(null);
    setContentError(null);
    setContentLoading(true);
    docxChapterContent(n.id)
      .then((c) => {
        if (reqIdRef.current === n.id) setContent(c);
      })
      .catch((e) => {
        if (reqIdRef.current === n.id) setContentError(normalizeError(e).message);
      })
      .finally(() => {
        if (reqIdRef.current === n.id) setContentLoading(false);
      });
  };

  const shown = useMemo(() => {
    if (!tree) return [];
    const kw = filter.trim();
    return kw ? filterTree(tree, kw.toLowerCase()) : tree;
  }, [tree, filter]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
    >
      <div
        className="flex max-h-[80vh] w-full max-w-4xl flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b px-4 py-3">
          <h3 className="min-w-0 flex-1 truncate text-sm font-medium" title={target.docName}>
            章节树 — {fileName(target.docName)}
          </h3>
          {tree && (
            <span className="shrink-0 text-xs text-muted-foreground">
              共 {countNodes(tree)} 个章节
            </span>
          )}
          <Button variant="ghost" size="icon-sm" className="shrink-0" title="关闭" onClick={onClose}>
            <X className="size-4" />
          </Button>
        </div>
        <div className="border-b px-4 py-2">
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={filter}
              placeholder="按编号 / 标题过滤章节"
              className="pl-8"
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
        </div>
        <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
          <div className="max-h-64 min-h-0 overflow-y-auto p-2 lg:max-h-none lg:w-[45%] lg:shrink-0 lg:border-r">
            {error ? (
              <p className="p-4 text-sm text-destructive">{error}</p>
            ) : !tree ? (
              <div className="flex justify-center py-10 text-muted-foreground">
                <Loader2 className="size-5 animate-spin" />
              </div>
            ) : shown.length === 0 ? (
              <p className="p-4 text-sm text-muted-foreground">
                {filter.trim() ? "无匹配章节" : "暂无章节数据"}
              </p>
            ) : (
              <TreeRows
                nodes={shown}
                depth={0}
                selectedId={selected?.id ?? null}
                onSelect={selectNode}
              />
            )}
          </div>
          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-4">
            {!selected ? (
              <p className="text-sm text-muted-foreground">点击左侧章节查看正文内容</p>
            ) : contentError ? (
              <p className="text-sm text-destructive">{contentError}</p>
            ) : contentLoading ? (
              <div className="flex justify-center py-10 text-muted-foreground">
                <Loader2 className="size-5 animate-spin" />
              </div>
            ) : (
              <>
                <h4 className="mb-2 shrink-0 text-sm font-medium">
                  {selected.number_prefix && (
                    <span className="mr-1.5 tabular-nums text-muted-foreground">
                      {selected.number_prefix}
                    </span>
                  )}
                  {selected.title}
                </h4>
                {content && content.trim() ? (
                  <div className="whitespace-pre-wrap text-sm leading-relaxed">{content}</div>
                ) : (
                  <p className="text-sm text-muted-foreground">该章节无正文内容</p>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
