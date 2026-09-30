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
  FileChips,
  OutcomeList,
  PathField,
  ProgressPanel,
  RunBar,
  pickDirSetter,
} from "@/components/common";
import { invoke, normalizeError, pickFiles } from "@/lib/tauri";
import { useTask } from "@/lib/task";
import type { FileOutcome } from "@/lib/types";

export default function PptxToPdf() {
  const [inputs, setInputs] = useState<string[]>([]);
  const [outDir, setOutDir] = useState("");
  const [outcomes, setOutcomes] = useState<FileOutcome[]>([]);
  const task = useTask("pptx-to-pdf");
  const [running, setRunning] = useState(false);

  const pickPptx = async () => {
    const files = await pickFiles("选择 PPTX 文件", ["pptx", "ppt"]);
    if (files.length > 0) {
      setInputs(files); // 重新选择时替换而非追加
    }
  };

  const run = async () => {
    if (inputs.length === 0) {
      toast.warning("请先选择要转换的文件");
      return;
    }
    setOutcomes([]);
    setRunning(true);
    try {
      const result = await invoke<FileOutcome[]>("pptx_to_pdf", {
        inputs,
        outDir: outDir || null,
      });
      setOutcomes(result);
      const ok = result.filter((r) => r.ok).length;
      toast.success(`转换完成：成功 ${ok} / ${result.length}`);
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  return (
    <ToolShell
      title="PPTX 转 PDF"
      description="把选中的 PPTX/PPT 文件批量转换为 PDF。需要本机安装 LibreOffice（可在设置页探测）。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入文件</CardTitle>
              <CardDescription>支持一次选择多个文件，转换顺序即列表顺序。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <PathField
                value={inputs}
                multiple
                placeholder="选择一个或多个 PPTX/PPT 文件"
                onPick={pickPptx}
              />
              <FileChips
                files={inputs}
                onRemove={(p) => setInputs((prev) => prev.filter((x) => x !== p))}
                onClear={() => setInputs([])}
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">输出目录</CardTitle>
              <CardDescription>不选择时，PDF 输出到各源文件所在目录。</CardDescription>
            </CardHeader>
            <CardContent>
              <PathField
                value={outDir ? [outDir] : []}
                placeholder="默认输出到源文件所在目录"
                onPick={pickDirSetter(setOutDir, "选择输出目录")}
              />
            </CardContent>
          </Card>
        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label={`开始转换${inputs.length > 0 ? `（${inputs.length} 个文件）` : ""}`}
            runningLabel="转换中…"
            hint="单文件转换失败不会中断批次，结束后在下方查看明细。"
          />

          <ProgressPanel task={task} onCancel={task.cancel} />
          <OutcomeList outcomes={outcomes} />
        </div>
      </div>
    </ToolShell>
  );
}
