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
  OutcomeList,
  PathField,
  ProgressPanel,
  RunBar,
  pickOneFileSetter,
} from "@/components/common";
import { invoke, normalizeError, pickFiles } from "@/lib/tauri";
import { useTask } from "@/lib/task";
import type { FileOutcome, WatermarkPosition, WatermarkParams } from "@/lib/types";

const POSITIONS: { value: WatermarkPosition; label: string }[] = [
  { value: "top_left", label: "上左" },
  { value: "top_center", label: "上中" },
  { value: "top_right", label: "上右" },
  { value: "middle_left", label: "中左" },
  { value: "center", label: "居中" },
  { value: "middle_right", label: "中右" },
  { value: "bottom_left", label: "下左" },
  { value: "bottom_center", label: "下中" },
  { value: "bottom_right", label: "下右" },
];

export default function Watermark() {
  const [inputs, setInputs] = useState<string[]>([]);
  const [mode, setMode] = useState<"text" | "image" | "tile">("text");
  const [outputMode, setOutputMode] = useState("suffix");

  // 文本水印参数
  const [text, setText] = useState("内部资料");
  const [fontSize, setFontSize] = useState(36);
  const [color, setColor] = useState("#FF0000");
  const [opacity, setOpacity] = useState(50);
  const [rotation, setRotation] = useState(-30);
  const [position, setPosition] = useState<WatermarkPosition>("center");
  const [spacingX, setSpacingX] = useState(0);
  const [spacingY, setSpacingY] = useState(0);

  // 图片水印参数
  const [wmPath, setWmPath] = useState("");
  const [scale, setScale] = useState(1);

  const [outcomes, setOutcomes] = useState<FileOutcome[]>([]);
  const task = useTask("watermark");
  const [running, setRunning] = useState(false);

  const pickImages = async () => {
    const files = await pickFiles("选择图片", ["png", "jpg", "jpeg", "webp", "bmp"]);
    if (files.length > 0) {
      setInputs(files); // 重新选择时替换而非追加
    }
  };

  const buildParams = (): WatermarkParams => {
    if (mode === "image") {
      return {
        type: "image",
        watermark_path: wmPath,
        scale,
        opacity: opacity / 100,
        rotation,
        position,
      };
    }
    return {
      type: mode,
      text,
      font_family: null,
      font_size: fontSize,
      color,
      opacity: opacity / 100,
      rotation,
      position,
      spacing_x: spacingX > 0 ? spacingX : null,
      spacing_y: spacingY > 0 ? spacingY : null,
    };
  };

  const run = async () => {
    if (inputs.length === 0) {
      toast.warning("请先选择要处理的图片");
      return;
    }
    if (mode === "image" && !wmPath) {
      toast.warning("请选择水印图片");
      return;
    }
    setOutcomes([]);
    setRunning(true);
    try {
      const result = await invoke<FileOutcome[]>("watermark_images", {
        inputs,
        params: buildParams(),
        outputMode,
      });
      setOutcomes(result);
      const ok = result.filter((r) => r.ok).length;
      toast.success(`处理完成：成功 ${ok} / ${result.length}`);
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  return (
    <ToolShell
      title="图片打水印"
      description="批量给图片加文字、图片或平铺水印。PNG 保留透明通道，JPEG/WebP 白底输出。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入图片</CardTitle>
              <CardDescription>支持一次选择多张图片，逐张处理。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <PathField
                value={inputs}
                multiple
                placeholder="选择一张或多张图片"
                onPick={pickImages}
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
              <CardTitle className="text-base">水印设置</CardTitle>
              <CardDescription>先选水印类型，再调整对应参数。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>水印类型</Label>
                  <Select value={mode} onValueChange={(v) => setMode(v as typeof mode)}>
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="text">文字水印</SelectItem>
                      <SelectItem value="image">图片水印</SelectItem>
                      <SelectItem value="tile">平铺水印</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1.5">
                  <Label>输出方式</Label>
                  <Select value={outputMode} onValueChange={(v) => setOutputMode(v ?? "suffix")}>
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="suffix">同目录加后缀（xx_wm.png）</SelectItem>
                      <SelectItem value="subdir">输出到 watermarked/ 子目录</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              {mode !== "image" ? (
                <>
                  <div className="space-y-1.5">
                    <Label>水印文字</Label>
                    <Input value={text} onChange={(e) => setText(e.target.value)} />
                  </div>
                  <div className="grid grid-cols-2 gap-4">
                    <div className="space-y-1.5">
                      <Label>字号</Label>
                      <Input
                        type="number"
                        min={8}
                        max={500}
                        value={fontSize}
                        onChange={(e) => setFontSize(Number(e.target.value))}
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
                  {mode === "tile" && (
                    <div className="grid grid-cols-2 gap-4">
                      <div className="space-y-1.5">
                        <Label>横向间距（0 = 自动）</Label>
                        <Input
                          type="number"
                          min={0}
                          value={spacingX}
                          onChange={(e) => setSpacingX(Number(e.target.value))}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label>纵向间距（0 = 自动）</Label>
                        <Input
                          type="number"
                          min={0}
                          value={spacingY}
                          onChange={(e) => setSpacingY(Number(e.target.value))}
                        />
                      </div>
                    </div>
                  )}
                </>
              ) : (
                <>
                  <div className="space-y-1.5">
                    <Label>水印图片（建议带透明通道的 PNG）</Label>
                    <PathField
                      value={wmPath ? [wmPath] : []}
                      placeholder="选择水印图片文件"
                      onPick={pickOneFileSetter(setWmPath, "选择水印图片", [
                        "png",
                        "jpg",
                        "jpeg",
                        "webp",
                      ])}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label>缩放：{scale.toFixed(2)}x</Label>
                    <Slider
                      min={0.05}
                      max={3}
                      step={0.05}
                      value={[scale]}
                      onValueChange={(v) => setScale(Array.isArray(v) ? v[0] : v)}
                    />
                  </div>
                </>
              )}

              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>不透明度：{opacity}%</Label>
                  <Slider
                    min={1}
                    max={100}
                    step={1}
                    value={[opacity]}
                    onValueChange={(v) => setOpacity(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>旋转：{rotation}°</Label>
                  <Slider
                    min={-180}
                    max={180}
                    step={1}
                    value={[rotation]}
                    onValueChange={(v) => setRotation(Array.isArray(v) ? v[0] : v)}
                  />
                </div>
              </div>

              {mode !== "tile" && (
                <div className="space-y-1.5">
                  <Label>位置</Label>
                  <Select
                    value={position}
                    onValueChange={(v) => setPosition(v as WatermarkPosition)}
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {POSITIONS.map((p) => (
                        <SelectItem key={p.value} value={p.value}>
                          {p.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              )}
            </CardContent>
          </Card>

        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label={`开始处理${inputs.length > 0 ? `（${inputs.length} 张图片）` : ""}`}
            runningLabel="处理中…"
            hint="处理完成后在下方结果列表中查看并打开输出文件。"
          />

          <ProgressPanel task={task} onCancel={task.cancel} />
          <OutcomeList outcomes={outcomes} />
        </div>
      </div>
    </ToolShell>
  );
}
