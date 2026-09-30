import { useState } from "react";
import { FolderOutput, Loader2, PlugZap, Wand2 } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { ToolShell } from "@/components/layout/ToolShell";
import {
  FileChips,
  fileName,
  parentOf,
  PathField,
  ProgressPanel,
  RunBar,
  TextArea,
  Toggle,
} from "@/components/common";
import { invoke, normalizeError, openDir, pickFiles, revealFile } from "@/lib/tauri";
import { formatDuration, useTask } from "@/lib/task";
import type { DifyProbe, PdfImagesOutcome } from "@/lib/types";

export default function PdfToImages() {
  const [inputs, setInputs] = useState<string[]>([]);
  const [dpi, setDpi] = useState(150);
  const [format, setFormat] = useState("jpeg");
  const [quality, setQuality] = useState(85);
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");

  // Dify 智能命名
  const [difyEnabled, setDifyEnabled] = useState(false);
  const [difyBaseUrl, setDifyBaseUrl] = useState("");
  const [difyApiKey, setDifyApiKey] = useState("");
  const [difyPrompt, setDifyPrompt] = useState("");
  const [probing, setProbing] = useState(false);
  const [probeResult, setProbeResult] = useState<DifyProbe | null>(null);

  const [outcomes, setOutcomes] = useState<PdfImagesOutcome[]>([]);
  const task = useTask("pdf-to-images");
  const [running, setRunning] = useState(false);

  const pickPdfs = async () => {
    const files = await pickFiles("选择 PDF 文件", ["pdf"]);
    if (files.length > 0) {
      setInputs(files); // 重新选择时替换而非追加
    }
  };

  const probe = async () => {
    if (!difyBaseUrl.trim() || !difyApiKey.trim() || !difyPrompt.trim()) {
      toast.warning("请先填写服务地址、API Key 和提示词");
      return;
    }
    setProbing(true);
    setProbeResult(null);
    try {
      const r = await invoke<DifyProbe>("probe_dify", {
        baseUrl: difyBaseUrl.trim(),
        apiKey: difyApiKey.trim(),
      });
      setProbeResult(r);
      if (r.ok) toast.success("Dify 接口连接正常");
    } catch {
      // invoke 封装已 toast
    } finally {
      setProbing(false);
    }
  };

  const numOrNull = (v: string) => {
    const n = Number(v);
    return Number.isInteger(n) && n > 0 ? n : null;
  };

  // Dify 三项必填 + 连接测试通过，才允许开始渲染。
  const difyInputsFilled =
    difyBaseUrl.trim().length > 0 &&
    difyApiKey.trim().length > 0 &&
    difyPrompt.trim().length > 0;
  const difyReady = !difyEnabled || (difyInputsFilled && probeResult?.ok === true);
  const difyBlockedReason = !difyInputsFilled
    ? "启用智能命名后，需填写服务地址、API Key 和提示词。"
    : "请先点击「测试连接」，连接成功后才能开始渲染。";

  const run = async () => {
    if (inputs.length === 0) {
      toast.warning("请先选择 PDF 文件");
      return;
    }
    if (difyEnabled) {
      if (!difyInputsFilled) {
        toast.warning("启用智能命名后，服务地址、API Key 和提示词均为必填");
        return;
      }
      if (probeResult?.ok !== true) {
        toast.warning("请先测试连接，连接成功后才能开始渲染");
        return;
      }
    }
    setOutcomes([]);
    setRunning(true);
    try {
      const result = await invoke<PdfImagesOutcome[]>("pdf_to_images", {
        inputs,
        dpi,
        format,
        quality,
        start: numOrNull(start),
        end: numOrNull(end),
        difyEnabled,
        difyBaseUrl: difyBaseUrl.trim(),
        difyApiKey: difyApiKey.trim(),
        difyPrompt: difyPrompt.trim(),
      });
      setOutcomes(result);
      const ok = result.filter((r) => r.ok).length;
      const cancelled = result.filter((r) => !r.ok && (r.error ?? "").includes("取消")).length;
      if (cancelled > 0 && ok === 0) toast.info("任务已取消");
      else if (ok === result.length) toast.success(`渲染完成：${result.length} 个 PDF`);
      else toast.warning(`部分完成：成功 ${ok} / ${result.length}`);
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  return (
    <ToolShell
      title="PDF 切图片"
      description="把 PDF 按页渲染为图片，图片按 0001、0002… 四位数字命名，每个 PDF 输出到独立目录。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入文件</CardTitle>
              <CardDescription>支持一次选择多个 PDF，将逐个渲染。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <PathField
                value={inputs}
                multiple
                placeholder="选择一个或多个 PDF 文件"
                onPick={pickPdfs}
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
              <CardTitle className="text-base">渲染参数</CardTitle>
              <CardDescription>页码范围对所有选中的 PDF 生效。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>输出格式</Label>
                  <Select value={format} onValueChange={(v) => setFormat(v ?? "png")}>
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="png">PNG（无损）</SelectItem>
                      <SelectItem value="jpeg">JPEG（有损）</SelectItem>
                      <SelectItem value="webp">WebP（有损）</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1.5">
                  <Label>DPI（72-600）</Label>
                  <Input
                    type="number"
                    min={72}
                    max={600}
                    value={dpi}
                    onChange={(e) => setDpi(Number(e.target.value))}
                  />
                </div>
              </div>

              {format !== "png" && (
                <div className="space-y-1.5">
                  <Label>质量：{quality}</Label>
                  <Slider
                    min={1}
                    max={100}
                    step={1}
                    value={[quality]}
                    onValueChange={(v) => setQuality(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
              )}

              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>起始页（可选）</Label>
                  <Input
                    type="number"
                    min={1}
                    placeholder="1"
                    value={start}
                    onChange={(e) => setStart(e.target.value)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>结束页（可选）</Label>
                  <Input
                    type="number"
                    min={1}
                    placeholder="最后一页"
                    value={end}
                    onChange={(e) => setEnd(e.target.value)}
                  />
                </div>
              </div>
            </CardContent>
          </Card>

        </div>

        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输出目录命名</CardTitle>
              <CardDescription>
                默认以 PDF 文件名命名输出目录（如 report.pdf → report/）；开启后由 Dify
                工作流根据文件名生成目录名，失败时自动回退为文件名。
              </CardDescription>
              <CardAction>
                <div className="flex items-center gap-2">
                  <span className="text-sm text-muted-foreground">Dify 智能命名</span>
                  <Toggle
                    checked={difyEnabled}
                    onCheckedChange={(v) => {
                      setDifyEnabled(v);
                      setProbeResult(null);
                    }}
                  />
                </div>
              </CardAction>
            </CardHeader>
            {difyEnabled && (
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-1.5">
                    <Label>
                      服务地址 <span className="text-destructive">*</span>
                    </Label>
                    <Input
                      value={difyBaseUrl}
                      placeholder="例如 https://dify.example.com/v1"
                      onChange={(e) => {
                        setDifyBaseUrl(e.target.value);
                        setProbeResult(null);
                      }}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label>
                      API Key <span className="text-destructive">*</span>
                    </Label>
                    <Input
                      type="password"
                      value={difyApiKey}
                      placeholder="app-xxxxxxxx"
                      onChange={(e) => {
                        setDifyApiKey(e.target.value);
                        setProbeResult(null);
                      }}
                    />
                  </div>
                </div>
                <div className="space-y-1.5">
                  <Label>
                    提取目录名的提示词 <span className="text-destructive">*</span>
                  </Label>
                  <TextArea
                    rows={3}
                    value={difyPrompt}
                    placeholder="作为 prompt 输入传给工作流，例如：请根据文件名提取一个简短、无特殊符号的中文目录名"
                    onChange={(e) => {
                      setDifyPrompt(e.target.value);
                      setProbeResult(null);
                    }}
                  />
                </div>
                <div className="flex min-h-7 flex-wrap items-center gap-3">
                  <Button variant="outline" size="sm" onClick={probe} disabled={probing}>
                    {probing ? (
                      <Loader2 className="size-3.5 animate-spin" />
                    ) : (
                      <PlugZap className="size-3.5" />
                    )}
                    测试连接
                  </Button>
                  {probeResult && (
                    <span
                      className={
                        probeResult.ok ? "text-sm text-emerald-600" : "text-sm text-destructive"
                      }
                    >
                      {probeResult.message}
                    </span>
                  )}
                </div>
                <p className="text-xs text-muted-foreground">
                  工作流入参：input_file_name（PDF 文件名）与 rule_prompt（上述提示词）；目录名取输出结构化字段的 clean_text 属性。
                </p>
              </CardContent>
            )}
          </Card>

          <RunBar
            running={running}
            onRun={run}
            runningLabel="切图中…"
            label={`开始渲染${inputs.length > 0 ? `（${inputs.length} 个 PDF）` : ""}`}
            disabled={!difyReady}
            disabledReason={difyBlockedReason}
            hint={
              difyEnabled
                ? "智能命名已启用：三个配置项填写完整且连接测试通过后即可渲染。"
                : "每个 PDF 的图片放入以其文件名命名的目录，按 0001 起四位数字命名。"
            }
          />

          <ProgressPanel task={task} onCancel={task.cancel} />

          {outcomes.length > 0 && <ResultList outcomes={outcomes} />}
        </div>
      </div>
    </ToolShell>
  );
}

function ResultList({ outcomes }: { outcomes: PdfImagesOutcome[] }) {
  const okCount = outcomes.filter((o) => o.ok).length;
  const firstDir = outcomes.find((o) => o.ok && o.output)?.output ?? null;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">
          渲染结果
          <span className="ml-2 text-sm font-normal text-muted-foreground">
            成功 {okCount} / {outcomes.length}
          </span>
        </CardTitle>
        <CardAction>
          {firstDir && (
            <Button variant="outline" size="sm" onClick={() => openDir(parentOf(firstDir))}>
              <FolderOutput className="size-4" />
              打开输出目录
            </Button>
          )}
        </CardAction>
      </CardHeader>
      <CardContent className="max-h-72 space-y-1.5 overflow-y-auto">
        {outcomes.map((o, i) => (
          <div
            key={`${o.input}-${i}`}
            className="flex min-h-7 items-center gap-2 text-sm"
            title={o.input}
          >
            {o.ok ? (
              <Badge variant="secondary" className="shrink-0">
                成功
              </Badge>
            ) : (
              <Badge variant="destructive" className="shrink-0">
                失败
              </Badge>
            )}
            <span className="min-w-0 flex-1 truncate">{fileName(o.input)}</span>
            {o.ok && (
              <>
                <Badge variant="outline" className="shrink-0">
                  {o.pages} 页
                </Badge>
                <Badge
                  variant={o.dir_source === "dify" ? "default" : "secondary"}
                  className="min-w-0 shrink-0"
                >
                  {o.dir_source === "dify" && <Wand2 className="size-3" />}
                  <span className="max-w-40 truncate">{o.dir_name}</span>
                </Badge>
                {formatDuration(o.duration_ms) && (
                  <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                    {formatDuration(o.duration_ms)}
                  </span>
                )}
              </>
            )}
            <span
              className={
                o.ok
                  ? "max-w-[26%] shrink-0 truncate text-right text-xs text-amber-600"
                  : "min-w-0 flex-1 truncate text-right text-xs text-destructive"
              }
              title={o.error ?? ""}
            >
              {o.error}
            </span>
            {o.ok && o.output && (
              <Button
                variant="ghost"
                size="icon-sm"
                title="在资源管理器中显示"
                onClick={() => revealFile(o.output!)}
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
