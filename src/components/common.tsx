import { type ComponentProps, type ReactNode, useEffect, useState } from "react";
import { Loader2, Play, Square, X } from "lucide-react";
import { cn } from "cn";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { FolderOpen, FolderOutput, Timer } from "lucide-react";
import { openDir, pickDirectory, pickFile, revealFile } from "@/lib/tauri";
import {
  formatBytes,
  formatDuration,
  STAGE_LABELS,
  type TaskState,
} from "@/lib/task";
import type { FileOutcome } from "@/lib/types";

/** 只读路径输入框 + 目录/文件选择按钮。 */
export function PathField({
  value,
  placeholder,
  onPick,
  multiple,
}: {
  value: string[];
  placeholder: string;
  onPick: () => void;
  multiple?: boolean;
}) {
  return (
    <div className="flex items-start gap-2">
      <div className="min-h-9 min-w-0 flex-1 rounded-md border bg-muted/40 px-3 py-1.5 text-sm">
        {value.length === 0 ? (
          <span className="text-muted-foreground">{placeholder}</span>
        ) : multiple ? (
          <div className="max-h-36 space-y-0.5 overflow-y-auto">
            {value.map((p) => (
              <div key={p} className="truncate" title={p}>
                {p}
              </div>
            ))}
          </div>
        ) : (
          <div className="truncate" title={value[0]}>
            {value[0]}
          </div>
        )}
      </div>
      <Button variant="outline" className="shrink-0" onClick={onPick}>
        <FolderOpen className="size-4" />
        选择
      </Button>
    </div>
  );
}

export function pickDirSetter(setValue: (v: string) => void, title: string) {
  return async () => {
    const dir = await pickDirectory(title);
    if (dir) setValue(dir);
  };
}

export function pickOneFileSetter(setValue: (v: string) => void, title: string, ext: string[]) {
  return async () => {
    const file = await pickFile(title, ext);
    if (file) setValue(file);
  };
}

/** 运行期间每 250ms 强制重渲染一次，让耗时显示平滑递增。 */
function useTicker(active: boolean) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setTick((x) => x + 1), 250);
    return () => clearInterval(t);
  }, [active]);
}

/** 进度面板：运行中显示进度条、实时耗时与取消按钮。 */
export function ProgressPanel({ task, onCancel }: { task: TaskState; onCancel: () => void }) {
  useTicker(task.running);
  if (!task.running) return null;
  const pct = task.total > 0 ? Math.round((task.current / task.total) * 100) : 0;
  // 事件间隔内本地补齐，避免耗时数字停顿。
  const elapsed = task.elapsedMs + Math.max(0, Date.now() - task.updatedAt);
  return (
    <Card>
      <CardContent className="flex items-center gap-3">
        <Loader2 className="size-4 shrink-0 animate-spin text-primary" />
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="flex items-baseline gap-3 text-sm">
            <span className="shrink-0 font-medium">{STAGE_LABELS[task.stage] ?? task.stage}</span>
            <span className="truncate text-muted-foreground" title={task.message ?? undefined}>
              {task.message ?? ""}
            </span>
            <span className="ml-auto flex shrink-0 items-center gap-2 tabular-nums text-muted-foreground">
              <span className="inline-flex items-center gap-1 text-xs">
                <Timer className="size-3.5" />
                {formatDuration(elapsed) || "0 s"}
              </span>
              <span>
                {task.current} / {task.total}
              </span>
            </span>
          </div>
          <Progress value={pct} />
        </div>
        <Button variant="outline" size="sm" className="shrink-0" onClick={onCancel}>
          <Square className="size-3.5" />
          取消
        </Button>
      </CardContent>
    </Card>
  );
}

/** 可移除的文件标签列表（配合多选 PathField 使用）。 */
export function FileChips({
  files,
  onRemove,
  onClear,
}: {
  files: string[];
  onRemove: (p: string) => void;
  onClear?: () => void;
}) {
  if (files.length === 0) return null;
  return (
    <div className="space-y-1.5">
      <div className="max-h-36 overflow-y-auto rounded-md">
        <div className="flex flex-wrap gap-1.5">
          {files.map((p) => (
            <Button key={p} variant="secondary" size="sm" title={p} onClick={() => onRemove(p)}>
              <span className="max-w-48 truncate">{fileName(p)}</span>
              <X className="size-3.5" />
            </Button>
          ))}
        </div>
      </div>
      {onClear && files.length > 1 && (
        <Button variant="ghost" size="xs" onClick={onClear}>
          清空列表（{files.length}）
        </Button>
      )}
    </div>
  );
}

/** 执行操作条：独立于参数分区的操作单元，承载主执行按钮。 */
export function RunBar({
  running,
  onRun,
  label,
  hint,
  runningLabel,
  disabled,
  disabledReason,
}: {
  running: boolean;
  onRun: () => void;
  label: string;
  hint?: string;
  runningLabel?: string;
  /** 运行之外的前置条件不满足时禁用按钮。 */
  disabled?: boolean;
  /** 禁用时的提示文案（也覆盖 hint 展示）。 */
  disabledReason?: string;
}) {
  const blocked = disabled === true;
  return (
    <div className="flex items-center justify-between gap-4 rounded-xl border bg-muted/30 px-4 py-3">
      <p className="min-w-0 text-sm text-muted-foreground">
        {blocked ? disabledReason : (hint ?? "确认上方参数无误后开始执行。")}
      </p>
      <Button
        size="lg"
        className="shrink-0"
        onClick={onRun}
        disabled={running || blocked}
        title={blocked ? disabledReason : undefined}
      >
        {running ? (
          <>
            <Loader2 className="size-4 animate-spin" />
            {runningLabel ?? "执行中…"}
          </>
        ) : (
          <>
            <Play className="size-4" />
            {label}
          </>
        )}
      </Button>
    </div>
  );
}

/** 开关（无依赖的轻量 Switch）。 */
export function Toggle({
  checked,
  onCheckedChange,
  disabled,
}: {
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        "inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-transparent transition-colors outline-none select-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50",
        checked ? "bg-primary" : "bg-input"
      )}
    >
      <span
        className={cn(
          "pointer-events-none block size-4 rounded-full bg-background shadow-sm transition-transform",
          checked ? "translate-x-[18px]" : "translate-x-0.5"
        )}
      />
    </button>
  );
}

/** 多行文本输入（与 Input 样式一致）。 */
export function TextArea({ className, ...props }: ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "min-h-16 w-full resize-y rounded-lg border border-input bg-transparent px-2.5 py-1.5 text-sm transition-colors outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:bg-input/30",
        className
      )}
      {...props}
    />
  );
}

/** 卡片分区标题：带步骤感的小标题。 */
export function SectionTitle({ children, hint }: { children: ReactNode; hint?: string }) {
  return (
    <div className="space-y-0.5">
      <div className="text-sm font-medium">{children}</div>
      {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
}

/** 批量结果列表。 */
export function OutcomeList({ outcomes }: { outcomes: FileOutcome[] }) {
  if (outcomes.length === 0) return null;
  const okCount = outcomes.filter((o) => o.ok).length;
  const outputs = outcomes.filter((o) => o.ok && o.output).map((o) => o.output!);
  const firstDir = outputs[0] ? parentOf(outputs[0]) : null;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">
          处理结果
          <span className="ml-2 text-sm font-normal text-muted-foreground">
            成功 {okCount} / {outcomes.length}
          </span>
        </CardTitle>
        <CardAction className="flex gap-2">
          {firstDir && (
            <Button variant="outline" size="sm" onClick={() => openDir(firstDir)}>
              <FolderOutput className="size-4" />
              打开输出目录
            </Button>
          )}
        </CardAction>
      </CardHeader>
      <CardContent className="max-h-72 space-y-1.5 overflow-y-auto">
        {outputs.length > 0 && outputs.length <= 8 && (
          <div className="mb-2 space-y-0.5 rounded-md bg-muted/40 p-2 text-xs text-muted-foreground">
            {outputs.map((p) => (
              <div
                key={p}
                className="cursor-pointer truncate hover:text-foreground"
                title="点击在资源管理器中显示"
                onClick={() => revealFile(p)}
              >
                {p}
              </div>
            ))}
          </div>
        )}
        {outcomes.map((o, i) => (
          <div key={`${o.input}-${i}`} className="flex items-center gap-2 text-sm">
            {o.ok ? (
              <Badge variant="secondary" className="shrink-0">
                成功
              </Badge>
            ) : (
              <Badge variant="destructive" className="shrink-0">
                失败
              </Badge>
            )}
            <span className="min-w-0 flex-1 truncate" title={o.input}>
              {fileName(o.input)}
            </span>
            {o.renamed && (
              <Badge variant="outline" className="shrink-0">
                重名已改名
              </Badge>
            )}
            {formatDuration(o.duration_ms) && (
              <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                {formatDuration(o.duration_ms)}
              </span>
            )}
            {o.ok ? (
              <span
                className="max-w-[28%] shrink-0 truncate text-xs text-muted-foreground"
                title={o.output ?? undefined}
              >
                {o.output}
              </span>
            ) : (
              <span
                className="max-w-[28%] shrink-0 truncate text-xs text-destructive"
                title={o.error ?? ""}
              >
                {o.error}
              </span>
            )}
            {o.ok && (
              <Button
                variant="ghost"
                size="icon-sm"
                className="shrink-0"
                title="在资源管理器中显示"
                onClick={() => o.output && revealFile(o.output)}
              >
                <FolderOutput className="size-3.5" />
              </Button>
            )}
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

/** 单结果摘要卡片（文件收集器 / PDF 切图）。 */
export function SummaryCard({
  title,
  destDir,
  rows,
}: {
  title: string;
  destDir: string;
  rows: { label: string; value: string }[];
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">{title}</CardTitle>
        <CardAction>
          <Button variant="outline" size="sm" onClick={() => openDir(destDir)}>
            <FolderOutput className="size-4" />
            打开输出目录
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="space-y-1 text-sm">
        {rows.map((r) => (
          <div key={r.label} className="flex justify-between gap-4">
            <span className="text-muted-foreground">{r.label}</span>
            <span className="text-right">{r.value}</span>
          </div>
        ))}
        <div className="flex justify-between gap-4">
          <span className="text-muted-foreground">输出目录</span>
          <span className="truncate text-right" title={destDir}>
            {destDir}
          </span>
        </div>
      </CardContent>
    </Card>
  );
}

export function formatCount(n: number): string {
  return `${n} 个文件`;
}

export { formatBytes };

export function parentOf(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : p;
}

export function fileName(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i >= 0 ? p.slice(i + 1) : p;
}
