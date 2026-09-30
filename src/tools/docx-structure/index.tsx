import { useCallback, useEffect, useState } from "react";
import { Database, ListTree } from "lucide-react";
import { toast } from "sonner";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ToolShell } from "@/components/layout/ToolShell";
import {
  FileChips,
  PathField,
  ProgressPanel,
  RunBar,
  Toggle,
  fileName,
} from "@/components/common";
import {
  docxDocuments,
  docxStructure,
  invoke,
  normalizeError,
  openDir,
  pickFiles,
} from "@/lib/tauri";
import { formatDuration, useTask } from "@/lib/task";
import type {
  DocxDocumentInfo,
  DocxOutcome,
  Settings,
} from "@/lib/types";
import ChapterTreeDialog, { type TreeTarget } from "./chapter-tree";

export default function DocxStructure() {
  const [inputs, setInputs] = useState<string[]>([]);
  const [llmEnabled, setLlmEnabled] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [outcomes, setOutcomes] = useState<DocxOutcome[]>([]);
  const [docs, setDocs] = useState<DocxDocumentInfo[]>([]);
  const [treeTarget, setTreeTarget] = useState<TreeTarget | null>(null);
  const task = useTask("docx-structure");
  const [running, setRunning] = useState(false);

  const refreshDocs = useCallback(async () => {
    try {
      setDocs(await docxDocuments());
    } catch {
      // invoke 封装已 toast（如尚未建库）。
    }
  }, []);

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then(setSettings)
      .catch(() => { });
    refreshDocs();
    // 设置页保存后热更新（keep-alive 架构下页面不会重新 mount）
    const onSettingsUpdated = () => {
      invoke<Settings>("get_settings")
        .then(setSettings)
        .catch(() => { });
    };
    window.addEventListener("settings-updated", onSettingsUpdated);
    return () => window.removeEventListener("settings-updated", onSettingsUpdated);
  }, [refreshDocs]);

  const pickDocx = async () => {
    const files = await pickFiles("选择 DOCX 文件", ["docx"]);
    if (files.length > 0) {
      setInputs(files); // 重新选择时替换而非追加
    }
  };

  const run = async () => {
    if (inputs.length === 0) {
      toast.warning("请先选择要拆分的 DOCX 文件");
      return;
    }
    setOutcomes([]);
    setRunning(true);
    try {
      const result = await docxStructure(inputs, llmEnabled);
      setOutcomes(result);
      const ok = result.filter((r) => r.ok).length;
      toast.success(`拆分完成：成功 ${ok} / ${result.length}`);
      refreshDocs();
    } catch (e) {
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
      task.markDone();
    }
  };

  const llmConfigured = Boolean(
    settings?.docx_llm_base_url?.trim() && settings?.docx_llm_api_key?.trim()
  );

  const openDbDir = () => {
    const dir = settings?.docx_db_dir?.trim();
    if (dir) {
      openDir(dir);
    } else {
      toast.info("数据库位于应用数据目录下的 docx_split\\docx_split.db", {
        description: "可在设置页「DOCX 结构化拆分」中自定义存储目录",
      });
    }
  };

  const okCount = outcomes.filter((o) => o.ok).length;

  return (
    <ToolShell
      title="DOCX 结构化拆分"
      description="把 DOCX 文档按章节层级拆分入库（SQLite + 全文索引），可选 LLM 结构恢复，支持章节树回看。"
    >
      <div className="mx-auto grid max-w-5xl items-start gap-4 lg:grid-cols-[minmax(0,1fr)_380px]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">输入文件</CardTitle>
              <CardDescription>支持一次选择多个 .docx 文件，按列表顺序串行拆分。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <PathField
                value={inputs}
                multiple
                placeholder="选择一个或多个 DOCX 文件"
                onPick={pickDocx}
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
              <CardTitle className="text-base">拆分参数</CardTitle>
              <CardDescription>
                关闭时使用规则模式建树（快速离线）；开启后对标题缺失的分段调用 LLM 恢复层级结构。
              </CardDescription>
              <CardAction>
                <div className="flex items-center gap-2">
                  <span className="text-sm text-muted-foreground">LLM 结构恢复</span>
                  <Toggle checked={llmEnabled} onCheckedChange={setLlmEnabled} />
                </div>
              </CardAction>
            </CardHeader>
            {llmEnabled && (
              <CardContent className="space-y-2">
                <div className="rounded-md bg-muted/40 px-3 py-2 text-xs">
                  <div className="truncate text-muted-foreground" title={settings?.docx_llm_base_url ?? ""}>
                    网关：{settings?.docx_llm_base_url?.trim() || "（未配置）"}
                  </div>
                  <div className="text-muted-foreground">
                    模型：{settings?.docx_llm_model?.trim() || "（未配置）"}
                  </div>
                </div>
                {!llmConfigured && (
                  <p className="text-xs text-amber-600 dark:text-amber-400">
                    未配置 LLM 网关，将使用规则模式建树。请到设置页填写 base_url 与 api_key。
                  </p>
                )}
              </CardContent>
            )}
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">历史文档</CardTitle>
              <CardDescription>已入库文档，可随时回看章节树。</CardDescription>
            </CardHeader>
            <CardContent className="max-h-72 space-y-1.5 overflow-y-auto">
              {docs.length === 0 ? (
                <p className="text-sm text-muted-foreground">暂无文档，先执行一次拆分。</p>
              ) : (
                docs.map((d) => (
                  <div key={d.id} className="flex items-center gap-2 text-sm">
                    <span className="min-w-0 flex-1 truncate" title={d.doc_name}>
                      {d.doc_name}
                    </span>
                    <Badge variant="outline" className="shrink-0">
                      {d.status}
                    </Badge>
                    {d.create_time && (
                      <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                        {d.create_time}
                      </span>
                    )}
                    <Button
                      variant="ghost"
                      size="sm"
                      className="shrink-0"
                      onClick={() => setTreeTarget({ docId: d.id, docName: d.doc_name })}
                    >
                      <ListTree className="size-3.5" />
                      章节树
                    </Button>
                  </div>
                ))
              )}
            </CardContent>
          </Card>
        </div>

        <div className="space-y-4">
          <RunBar
            running={running}
            onRun={run}
            label={`开始拆分${inputs.length > 0 ? `（${inputs.length} 个文件）` : ""}`}
            runningLabel="拆分中…"
            hint="单文件拆分失败不会中断批次；大文档建议开启 LLM 结构恢复获得更准的章节层级。"
          />

          <ProgressPanel task={task} onCancel={task.cancel} />

          {outcomes.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">
                  拆分结果
                  <span className="ml-2 text-sm font-normal text-muted-foreground">
                    成功 {okCount} / {outcomes.length}
                  </span>
                </CardTitle>
                <CardAction>
                  <Button variant="outline" size="sm" onClick={openDbDir}>
                    <Database className="size-4" />
                    查看数据库
                  </Button>
                </CardAction>
              </CardHeader>
              <CardContent className="max-h-96 space-y-1.5 overflow-y-auto">
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
                    {o.ok && (
                      <span className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
                        章节 {o.chapters ?? 0} · 表格 {o.tables ?? 0} · 待复核 {o.needs_review ?? 0}
                      </span>
                    )}
                    {formatDuration(o.duration_ms) && (
                      <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                        {formatDuration(o.duration_ms)}
                      </span>
                    )}
                    {o.ok ? (
                      o.doc_id != null && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="shrink-0"
                          onClick={() =>
                            setTreeTarget({ docId: o.doc_id!, docName: fileName(o.input) })
                          }
                        >
                          <ListTree className="size-3.5" />
                          章节树
                        </Button>
                      )
                    ) : (
                      <span
                        className="max-w-[32%] shrink-0 truncate text-xs text-destructive"
                        title={o.error ?? ""}
                      >
                        {o.error}
                      </span>
                    )}
                  </div>
                ))}
              </CardContent>
            </Card>
          )}
        </div>
      </div>

      {treeTarget && (
        <ChapterTreeDialog target={treeTarget} onClose={() => setTreeTarget(null)} />
      )}
    </ToolShell>
  );
}
