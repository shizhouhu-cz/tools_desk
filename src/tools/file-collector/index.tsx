import { useState } from "react";
import { toast } from "sonner";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ToolShell } from "@/components/layout/ToolShell";
import {
  PathField,
  ProgressPanel,
  RunBar,
  SummaryCard,
  pickDirSetter,
} from "@/components/common";
import { invoke, normalizeError } from "@/lib/tauri";
import { formatBytes, useTask } from "@/lib/task";
import type { CollectSummary } from "@/lib/types";

export default function FileCollector() {
  const [dir, setDir] = useState("");
  const [summary, setSummary] = useState<CollectSummary | null>(null);
  const task = useTask("file-collector");
  const [running, setRunning] = useState(false);

  const run = async () => {
    if (!dir) {
      toast.warning("请先选择源目录");
      return;
    }
    setSummary(null);
    setRunning(true);
    try {
      const result = await invoke<CollectSummary>("collect_files", { sourceDir: dir });
      setSummary(result);
      toast.success(`已收集 ${result.files} 个文件`);
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  return (
    <ToolShell
      title="文件收集器"
      description="选定目录后，在该目录下新建临时目录，把目录（含子目录）中的全部文件复制进去。"
    >
      <div className="mx-auto max-w-2xl space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">源目录</CardTitle>
            <CardDescription>
              将在源目录下创建 _collected_时间戳 目录；跨子目录同名文件自动加序号；收集过程可取消，取消后清理目标目录。
            </CardDescription>
          </CardHeader>
          <CardContent>
            <PathField
              value={dir ? [dir] : []}
              placeholder="选择要收集文件的目录"
              onPick={pickDirSetter(setDir, "选择源目录")}
            />
          </CardContent>
        </Card>

        <RunBar
          running={running}
          onRun={run}
          label="开始收集"
          runningLabel="收集中…"
          hint={dir ? `将收集 ${dir} 下的全部文件（含子目录）。` : "先选择源目录。"}
        />

        <ProgressPanel task={task} onCancel={task.cancel} />

        {summary && (
          <SummaryCard
            title="收集完成"
            destDir={summary.dest_dir}
            rows={[
              { label: "文件数", value: `${summary.files}` },
              { label: "总大小", value: formatBytes(summary.bytes) },
              { label: "改名文件", value: `${summary.renamed}` },
            ]}
          />
        )}
      </div>
    </ToolShell>
  );
}
