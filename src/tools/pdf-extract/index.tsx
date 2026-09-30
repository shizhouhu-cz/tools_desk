import { useState } from "react";
import { toast } from "sonner";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ToolShell } from "@/components/layout/ToolShell";
import { OutcomeList, PathField, RunBar, fileName, parentOf } from "@/components/common";
import { invoke, normalizeError, pickFile, pickSaveFile } from "@/lib/tauri";
import type { FileOutcome } from "@/lib/types";

/** 由输入路径推导默认输出路径：同目录 `{stem}_extract.pdf`。 */
function defaultOutput(input: string): string {
  const sep = input.includes("\\") ? "\\" : "/";
  const name = fileName(input);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${parentOf(input)}${sep}${stem}_extract.pdf`;
}

export default function PdfExtract() {
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [startPage, setStartPage] = useState(1);
  const [endPage, setEndPage] = useState(1);

  const [outcomes, setOutcomes] = useState<FileOutcome[]>([]);
  const [running, setRunning] = useState(false);

  const pickPdf = async () => {
    const file = await pickFile("选择 PDF 文件", ["pdf"]);
    if (file) {
      setInput(file);
      setOutput(defaultOutput(file));
      setOutcomes([]);
    }
  };

  const saveAs = async () => {
    const fallbackName = input ? defaultOutput(input) : "extracted.pdf";
    const path = await pickSaveFile("提取结果另存为", fallbackName, ["pdf"]);
    if (path) setOutput(path);
  };

  const run = async () => {
    if (!input) {
      toast.warning("请先选择 PDF 文件");
      return;
    }
    if (!output) {
      toast.warning("请选择输出文件位置");
      return;
    }
    if (!Number.isInteger(startPage) || !Number.isInteger(endPage) || startPage < 1) {
      toast.warning("页码必须是不小于 1 的整数");
      return;
    }
    if (startPage > endPage) {
      toast.warning("起始页不能大于结束页");
      return;
    }
    setOutcomes([]);
    setRunning(true);
    const started = Date.now();
    try {
      const result = await invoke<{ output: string; pages: number }>("pdf_extract_pages", {
        input,
        output,
        startPage,
        endPage,
      });
      setOutcomes([
        {
          input,
          ok: true,
          output: result.output,
          error: null,
          renamed: false,
          duration_ms: Date.now() - started,
        },
      ]);
      toast.success(`提取完成：${result.pages} 页`);
    } catch (e) {
      if (normalizeError(e).code !== "cancelled") {
        setOutcomes([
          { input, ok: false, output: null, error: normalizeError(e).message, renamed: false },
        ]);
      }
    } finally {
      setRunning(false);
    }
  };

  return (
    <ToolShell
      title="PDF 页面提取"
      description="从 PDF 中提取指定页码范围，生成新的 PDF 文件，单文件另存输出。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入 / 输出文件</CardTitle>
              <CardDescription>选择 PDF 后自动在同目录生成默认输出名，可另存修改。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="space-y-1.5">
                <Label>输入 PDF</Label>
                <PathField
                  value={input ? [input] : []}
                  placeholder="选择一个 PDF 文件"
                  onPick={pickPdf}
                />
              </div>
              <div className="space-y-1.5">
                <Label>输出 PDF（另存为）</Label>
                <PathField value={output ? [output] : []} placeholder="选择输出位置" onPick={saveAs} />
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">页码范围</CardTitle>
              <CardDescription>页码从 1 开始。范围越界或文件加密时会给出明确错误提示。</CardDescription>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>起始页</Label>
                  <Input
                    type="number"
                    min={1}
                    value={startPage}
                    onChange={(e) => setStartPage(Math.floor(Number(e.target.value)))}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>结束页</Label>
                  <Input
                    type="number"
                    min={1}
                    value={endPage}
                    onChange={(e) => setEndPage(Math.floor(Number(e.target.value)))}
                  />
                </div>
              </div>
            </CardContent>
          </Card>
        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label={`开始提取（第 ${startPage}-${endPage} 页）`}
            runningLabel="提取中…"
            hint="提取只重写页面树引用，不重新编码页面内容，速度快且不损失质量。"
          />
          <OutcomeList outcomes={outcomes} />
        </div>
      </div>
    </ToolShell>
  );
}
