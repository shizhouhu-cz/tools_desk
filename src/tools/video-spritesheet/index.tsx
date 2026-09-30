import { useState } from "react";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { Film, FolderOutput } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ToolShell } from "@/components/layout/ToolShell";
import {
  PathField,
  ProgressPanel,
  RunBar,
  pickOneFileSetter,
} from "@/components/common";
import { formatDuration } from "@/lib/task";
import { normalizeError, openDir, pickDirectory, revealFile, videoSpritesheet } from "@/lib/tauri";
import { useTask } from "@/lib/task";
import type { SpriteOutcome } from "@/lib/types";

const VIDEO_EXTS = ["mp4", "mkv", "mov", "webm", "avi", "m4v", "flv", "ts", "wmv"];

export default function VideoSpritesheet() {
  const [videoPath, setVideoPath] = useState("");
  const [fps, setFps] = useState(24);
  const [maxWidth, setMaxWidth] = useState(480);
  const [sheetSize, setSheetSize] = useState(4096);
  const [format, setFormat] = useState<"jpeg" | "png">("jpeg");
  const [quality, setQuality] = useState(85);
  const [maxFrames, setMaxFrames] = useState(2000);
  const [outDir, setOutDir] = useState("");

  const [outcome, setOutcome] = useState<SpriteOutcome | null>(null);
  const task = useTask("video-spritesheet");
  const [running, setRunning] = useState(false);

  const run = async () => {
    if (!videoPath) {
      toast.warning("请先选择视频文件");
      return;
    }
    setOutcome(null);
    setRunning(true);
    try {
      const r = await videoSpritesheet({
        input: videoPath,
        fps,
        maxFrameWidth: maxWidth,
        maxSheetSize: sheetSize,
        format,
        quality,
        maxFrames,
        outputDir: outDir || null,
      });
      setOutcome(r);
      if (r.warning) {
        toast.warning(r.warning);
      } else {
        toast.success(`精灵图生成完成：${r.frame_count} 帧 / ${r.pages} 张图集`);
      }
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  return (
    <ToolShell
      title="视频转精灵图"
      description="把视频按采样帧率抽帧，自动计算网格布局生成精灵图集，打包为含自包含播放器的 zip；解压后双击 player.html 即可播放。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入视频</CardTitle>
              <CardDescription>支持 MP4 / MKV / MOV / WebM 等常见格式（由内置 ffmpeg 解码）。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <PathField
                value={videoPath ? [videoPath] : []}
                placeholder="选择一个视频文件"
                onPick={pickOneFileSetter(setVideoPath, "选择视频文件", VIDEO_EXTS)}
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">参数设置</CardTitle>
              <CardDescription>默认参数以流畅优先；帧数超上限时自动降低采样帧率并显式提示。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1.5">
                  <Label>采样帧率（fps）</Label>
                  <Input
                    type="number"
                    min={1}
                    max={240}
                    value={fps}
                    onChange={(e) => setFps(Number(e.target.value))}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>单帧最大宽度（px）</Label>
                  <Input
                    type="number"
                    min={16}
                    max={4096}
                    value={maxWidth}
                    onChange={(e) => setMaxWidth(Number(e.target.value))}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>图集边长上限（px）</Label>
                  <Input
                    type="number"
                    min={256}
                    max={16384}
                    value={sheetSize}
                    onChange={(e) => setSheetSize(Number(e.target.value))}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>帧数上限（内存保护阀）</Label>
                  <Input
                    type="number"
                    min={1}
                    max={100000}
                    value={maxFrames}
                    onChange={(e) => setMaxFrames(Number(e.target.value))}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>图集格式</Label>
                  <Select value={format} onValueChange={(v) => setFormat(v as typeof format)}>
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="jpeg">JPEG（体积小，推荐）</SelectItem>
                      <SelectItem value="png">PNG（无损，体积大）</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                {format === "jpeg" && (
                  <div className="space-y-1.5">
                    <Label>JPEG 质量</Label>
                    <Input
                      type="number"
                      min={1}
                      max={100}
                      value={quality}
                      onChange={(e) => setQuality(Number(e.target.value))}
                    />
                  </div>
                )}
              </div>
              <div className="space-y-1.5">
                <Label>输出目录（留空则输出到视频同目录）</Label>
                <PathField
                  value={outDir ? [outDir] : []}
                  placeholder="视频同目录"
                  onPick={async () => {
                    const dir = await pickDirectory("选择输出目录");
                    if (dir) setOutDir(dir);
                  }}
                />
              </div>
            </CardContent>
          </Card>

          {outcome && (
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2 text-base">
                  生成结果
                  {outcome.downsampled && (
                    <Badge variant="outline" className="text-amber-600 dark:text-amber-400">
                      已降级 {outcome.requested_fps.toFixed(2)} → {outcome.actual_fps.toFixed(2)} fps
                    </Badge>
                  )}
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-3">
                <div className="space-y-1 text-sm">
                  <Row label="输出文件">
                    <button
                      className="truncate hover:underline"
                      title="点击在资源管理器中显示"
                      onClick={() => revealFile(outcome.zip_path)}
                    >
                      {outcome.zip_path}
                    </button>
                  </Row>
                  <Row label="实际帧率">{outcome.actual_fps.toFixed(2)} fps（请求 {outcome.requested_fps.toFixed(2)}）</Row>
                  <Row label="帧数">
                    {outcome.frame_count} 帧（单帧 {outcome.frame_width}×{outcome.frame_height}）
                  </Row>
                  <Row label="图集">
                    {outcome.pages} 页 × {outcome.cols} 列
                  </Row>
                  <Row label="耗时">{formatDuration(outcome.duration_ms)}</Row>
                </div>
                <div className="flex gap-2">
                  <button
                    className="inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm hover:bg-accent"
                    onClick={() => openDir(zipDir(outcome.zip_path))}
                  >
                    <FolderOutput className="size-4" />
                    打开输出目录
                  </button>
                </div>
                <div className="overflow-hidden rounded-lg border">
                  <iframe
                    key={outcome.preview_url}
                    src={outcome.preview_url}
                    title="精灵图预览"
                    className="h-[480px] w-full bg-black"
                  />
                </div>
                <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Film className="size-3.5" />
                  预览加载的即为 zip 产物中的 data.js / player.html / 精灵图。
                </p>
              </CardContent>
            </Card>
          )}
        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label="开始生成"
            runningLabel="生成中…"
            hint="生成完成后可在结果区直接预览播放效果，zip 解压后双击 player.html 播放。"
          />
          <ProgressPanel task={task} onCancel={task.cancel} />
        </div>
      </div>
    </ToolShell>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex justify-between gap-4">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 text-right">{children}</span>
    </div>
  );
}

function zipDir(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : p;
}
