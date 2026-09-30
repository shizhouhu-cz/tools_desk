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
import { Slider } from "@/components/ui/slider";
import { ToolShell } from "@/components/layout/ToolShell";
import { OutcomeList, PathField, RunBar, fileName, parentOf } from "@/components/common";
import { invoke, normalizeError, pickFile, pickSaveFile } from "@/lib/tauri";
import type { FileOutcome, PdfWatermarkParams } from "@/lib/types";

/** 由输入路径推导默认输出路径：同目录 `{stem}_watermark.pdf`。 */
function defaultOutput(input: string): string {
  const sep = input.includes("\\") ? "\\" : "/";
  const name = fileName(input);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${parentOf(input)}${sep}${stem}_watermark.pdf`;
}

export default function PdfWatermark() {
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");

  const [text, setText] = useState("内部资料");
  const [fontSize, setFontSize] = useState(48);
  const [color, setColor] = useState("#808080");
  const [opacity, setOpacity] = useState(30);
  const [angle, setAngle] = useState(45);
  const [density, setDensity] = useState(1);

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
    const fallbackName = input ? defaultOutput(input) : "watermarked.pdf";
    const path = await pickSaveFile("水印输出另存为", fallbackName, ["pdf"]);
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
    if (!text.trim()) {
      toast.warning("请输入水印文字");
      return;
    }
    setOutcomes([]);
    setRunning(true);
    const started = Date.now();
    try {
      const params: PdfWatermarkParams = {
        text,
        font_size: fontSize,
        color,
        opacity: opacity / 100,
        angle,
        density,
      };
      const result = await invoke<{ output: string; pages: number }>("pdf_watermark", {
        input,
        output,
        params,
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
      toast.success(`水印完成：${result.pages} 页`);
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
      title="PDF 打水印"
      description="给 PDF 每页添加半透明斜向平铺文字水印，直接嵌入系统中文字体，单文件另存输出。"
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
              <CardTitle className="text-base">水印设置</CardTitle>
              <CardDescription>水印以平铺方式覆盖每一页，相邻行交错排列。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-1.5">
                <Label>水印文字</Label>
                <Input value={text} onChange={(e) => setText(e.target.value)} />
              </div>
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>字号：{fontSize}</Label>
                  <Slider
                    min={12}
                    max={120}
                    step={1}
                    value={[fontSize]}
                    onValueChange={(v) => setFontSize(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>颜色</Label>
                  <div className="flex h-8 items-center gap-2 rounded-lg border px-2.5">
                    <input
                      type="color"
                      value={color}
                      onChange={(e) => setColor(e.target.value)}
                      className="size-5 cursor-pointer rounded border-0 bg-transparent p-0"
                    />
                    <span className="text-sm text-muted-foreground">{color}</span>
                  </div>
                </div>
              </div>
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>不透明度：{opacity}%</Label>
                  <Slider
                    min={5}
                    max={100}
                    step={1}
                    value={[opacity]}
                    onValueChange={(v) => setOpacity(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>旋转角度：{angle}°</Label>
                  <Slider
                    min={-90}
                    max={90}
                    step={1}
                    value={[angle]}
                    onValueChange={(v) => setAngle(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
              </div>
              <div className="space-y-1.5">
                <Label>密度：{density.toFixed(1)}x</Label>
                <Slider
                  min={0.5}
                  max={5}
                  step={0.1}
                  value={[density]}
                  onValueChange={(v) => setDensity(Array.isArray(v) ? v[0] : v)}
                />
              </div>
            </CardContent>
          </Card>
        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label="开始打水印"
            runningLabel="处理中…"
            hint="处理完成后可点击结果中的路径在资源管理器中定位输出文件。"
          />
          <OutcomeList outcomes={outcomes} />
        </div>
      </div>
    </ToolShell>
  );
}
