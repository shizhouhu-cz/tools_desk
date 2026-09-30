import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
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
import { invoke, pickDirectory, pickFile } from "@/lib/tauri";
import type { Settings, SofficeProbe } from "@/lib/types";

export default function SettingsPage() {
  const [s, setS] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [probing, setProbing] = useState(false);

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then(setS)
      .catch(() => { });
  }, []);

  const upd = (patch: Partial<Settings>) => setS((p) => (p ? { ...p, ...patch } : p));
  const num = (v: string) => (Number.isFinite(Number(v)) ? Number(v) : 0);

  const save = async (): Promise<Settings | null> => {
    if (!s) return null;
    setSaving(true);
    try {
      const saved = await invoke<Settings>("set_settings", { newSettings: { ...s } });
      setS(saved);
      // 通知 keep-alive 常驻页面热更新配置
      window.dispatchEvent(new CustomEvent("settings-updated"));
      toast.success("设置已保存");
      return saved;
    } catch {
      return null;
    } finally {
      setSaving(false);
    }
  };

  const probe = async () => {
    const saved = await save();
    if (!saved) return;
    setProbing(true);
    try {
      const r = await invoke<SofficeProbe>("probe_soffice");
      if (r.ok) toast.success("LibreOffice 探测成功，soffice 可用");
      else toast.error("soffice 探测失败", { description: "请检查 LibreOffice 安装与路径配置" });
    } catch {
      // invoke 封装已 toast
    } finally {
      setProbing(false);
    }
  };

  if (!s) {
    return (
      <ToolShell title="设置" description="加载中...">
        <div className="flex justify-center py-16 text-muted-foreground">
          <Loader2 className="size-5 animate-spin" />
        </div>
      </ToolShell>
    );
  }

  return (
    <ToolShell title="设置" description="工具运行参数，保存后立即生效并持久化到本机配置。">
      <div className="mx-auto grid max-w-5xl items-start gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">LibreOffice（PPTX 转 PDF）</CardTitle>
            <CardDescription>
              留空 soffice 路径时将按常见安装位置自动探测。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1.5">
              <Label>soffice 可执行文件路径</Label>
              <div className="flex gap-2">
                <Input
                  value={s.soffice_path}
                  placeholder="例如 C:/Program Files/LibreOffice/program/soffice.com"
                  onChange={(e) => upd({ soffice_path: e.target.value })}
                />
                <Button
                  variant="outline"
                  className="shrink-0"
                  onClick={async () => {
                    const f = await pickFile("选择 soffice 可执行文件", ["exe", "com", "bat"]);
                    if (f) upd({ soffice_path: f });
                  }}
                >
                  选择
                </Button>
                <Button className="shrink-0" onClick={probe} disabled={saving || probing}>
                  {probing && <Loader2 className="size-4 animate-spin" />}
                  保存并探测
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label>转换单文件超时（秒，5-3600）</Label>
                <Input
                  type="number"
                  min={5}
                  max={3600}
                  value={s.libreoffice_timeout_sec}
                  onChange={(e) => upd({ libreoffice_timeout_sec: num(e.target.value) })}
                />
              </div>
              <div className="space-y-1.5">
                <Label>并发转换数（1-8）</Label>
                <Input
                  type="number"
                  min={1}
                  max={8}
                  value={s.libreoffice_concurrency}
                  onChange={(e) => upd({ libreoffice_concurrency: num(e.target.value) })}
                />
              </div>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">PDF 渲染（PDF 切图片）</CardTitle>
            <CardDescription>
              pdfium.dll 默认随应用打包，仅在需要使用其他版本时手动指定。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1.5">
              <Label>pdfium.dll 路径（可选）</Label>
              <div className="flex gap-2">
                <Input
                  value={s.pdfium_path}
                  placeholder="留空使用内置 pdfium.dll"
                  onChange={(e) => upd({ pdfium_path: e.target.value })}
                />
                <Button
                  variant="outline"
                  className="shrink-0"
                  onClick={async () => {
                    const f = await pickFile("选择 pdfium.dll", ["dll"]);
                    if (f) upd({ pdfium_path: f });
                  }}
                >
                  选择
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-3 gap-4">
              <div className="space-y-1.5">
                <Label>默认 DPI（72-600）</Label>
                <Input
                  type="number"
                  min={72}
                  max={600}
                  value={s.default_dpi}
                  onChange={(e) => upd({ default_dpi: num(e.target.value) })}
                />
              </div>
              <div className="space-y-1.5">
                <Label>默认质量（1-100）</Label>
                <Input
                  type="number"
                  min={1}
                  max={100}
                  value={s.default_quality}
                  onChange={(e) => upd({ default_quality: num(e.target.value) })}
                />
              </div>
              <div className="space-y-1.5">
                <Label>最大页数（1-2000）</Label>
                <Input
                  type="number"
                  min={1}
                  max={2000}
                  value={s.max_pages}
                  onChange={(e) => upd({ max_pages: num(e.target.value) })}
                />
              </div>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">水印字体（图片打水印）</CardTitle>
            <CardDescription>
              文字水印使用本地字体文件渲染，默认字体名用于在同目录多种字体中选择。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1.5">
              <Label>字体目录</Label>
              <div className="flex gap-2">
                <Input
                  value={s.font_dir}
                  placeholder="例如 C:/Windows/Fonts"
                  onChange={(e) => upd({ font_dir: e.target.value })}
                />
                <Button
                  variant="outline"
                  className="shrink-0"
                  onClick={async () => {
                    const d = await pickDirectory("选择字体目录");
                    if (d) upd({ font_dir: d });
                  }}
                >
                  选择
                </Button>
              </div>
            </div>
            <div className="space-y-1.5">
              <Label>默认字体名</Label>
              <Input
                value={s.default_font}
                placeholder="例如 SimHei（黑体）"
                onChange={(e) => upd({ default_font: e.target.value })}
              />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">DOCX 结构化拆分</CardTitle>
            <CardDescription>
              Python 运行时与数据库默认随应用内置（应用数据目录 docx_split），LLM 用于恢复缺失的章节层级。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1.5">
              <Label>Python 解释器路径（可选）</Label>
              <div className="flex gap-2">
                <Input
                  value={s.docx_python_path}
                  placeholder="留空使用内置 Python 运行时"
                  onChange={(e) => upd({ docx_python_path: e.target.value })}
                />
                <Button
                  variant="outline"
                  className="shrink-0"
                  onClick={async () => {
                    const f = await pickFile("选择 python.exe", ["exe"]);
                    if (f) upd({ docx_python_path: f });
                  }}
                >
                  选择
                </Button>
              </div>
            </div>
            <div className="space-y-1.5">
              <Label>数据库存储目录（可选）</Label>
              <div className="flex gap-2">
                <Input
                  value={s.docx_db_dir}
                  placeholder="留空使用应用数据目录下的 docx_split"
                  onChange={(e) => upd({ docx_db_dir: e.target.value })}
                />
                <Button
                  variant="outline"
                  className="shrink-0"
                  onClick={async () => {
                    const d = await pickDirectory("选择数据库存储目录");
                    if (d) upd({ docx_db_dir: d });
                  }}
                >
                  选择
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label>LLM Provider</Label>
                <Input
                  value={s.docx_llm_provider}
                  placeholder="openai 或 mock"
                  onChange={(e) => upd({ docx_llm_provider: e.target.value })}
                />
              </div>
              <div className="space-y-1.5">
                <Label>模型名</Label>
                <Input
                  value={s.docx_llm_model}
                  placeholder="例如 qwen-plus"
                  onChange={(e) => upd({ docx_llm_model: e.target.value })}
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <Label>LLM 网关地址（OpenAI 兼容 /chat/completions）</Label>
              <Input
                value={s.docx_llm_base_url}
                placeholder="例如 https://dashscope.example.com/compatible-mode/v1"
                onChange={(e) => upd({ docx_llm_base_url: e.target.value })}
              />
            </div>
            <div className="space-y-1.5">
              <Label>API Key</Label>
              <Input
                type="password"
                value={s.docx_llm_api_key}
                placeholder="sk-..."
                onChange={(e) => upd({ docx_llm_api_key: e.target.value })}
              />
            </div>
            <div className="space-y-1.5">
              <Label>认证头名（可选）</Label>
              <Input
                value={s.docx_llm_auth_header}
                placeholder="留空使用 Authorization: Bearer；内网网关可自定义"
                onChange={(e) => upd({ docx_llm_auth_header: e.target.value })}
              />
            </div>
          </CardContent>
        </Card>

        <div className="sticky bottom-0 -mx-6 flex justify-end border-t bg-background px-6 py-3 md:col-span-2">
          <Button onClick={save} disabled={saving}>
            {saving && <Loader2 className="size-4 animate-spin" />}
            保存设置
          </Button>
        </div>
      </div>
    </ToolShell>
  );
}
