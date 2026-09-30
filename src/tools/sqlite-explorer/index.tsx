import { useCallback, useState, type KeyboardEvent } from "react";
import {
  Database,
  FolderOpen,
  Loader2,
  Play,
  RefreshCw,
  Table2,
  Unplug,
} from "lucide-react";
import { toast } from "sonner";
import { cn } from "cn";
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
import { fileName } from "@/components/common";
import {
  normalizeError,
  pickFile,
  sqliteDetect,
  sqliteQuery,
  sqliteScan,
  sqliteTables,
} from "@/lib/tauri";
import type { SqliteQueryResult, SqliteTableInfo } from "@/lib/types";

/** 表名转 SQL 标识符：双引号包裹并转义内部双引号。 */
function quoteIdent(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}

export default function SqliteExplorer() {
  const [candidates, setCandidates] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [connected, setConnected] = useState<string | null>(null);
  const [tables, setTables] = useState<SqliteTableInfo[]>([]);
  const [tablesLoading, setTablesLoading] = useState(false);
  const [sql, setSql] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<SqliteQueryResult | null>(null);

  const scan = async () => {
    setScanning(true);
    try {
      setCandidates(await sqliteScan());
    } catch {
      // invoke 封装已 toast。
    } finally {
      setScanning(false);
    }
  };

  // 连接数据库并列出表；失败时回退到未连接状态（错误 toast 由 invoke 封装处理）。
  const connect = useCallback(async (path: string) => {
    setConnected(path);
    setTables([]);
    setResult(null);
    setTablesLoading(true);
    try {
      setTables(await sqliteTables(path));
    } catch {
      setConnected(null);
    } finally {
      setTablesLoading(false);
    }
  }, []);

  const pickDb = async () => {
    const p = await pickFile("选择 SQLite 数据库", ["db", "sqlite", "sqlite3", "db3"]);
    if (!p) return;
    let ok = false;
    try {
      ok = await sqliteDetect(p);
    } catch {
      return; // 文件不存在等错误已由 invoke 封装 toast。
    }
    if (!ok) {
      toast.error("不是有效的 SQLite 数据库文件");
      return;
    }
    connect(p);
  };

  const disconnect = () => {
    setConnected(null);
    setTables([]);
    setResult(null);
    setSql("");
  };

  const run = async () => {
    if (!connected) {
      toast.warning("请先连接数据库");
      return;
    }
    if (!sql.trim()) {
      toast.warning("请输入 SQL 语句");
      return;
    }
    setRunning(true);
    try {
      setResult(await sqliteQuery(connected, sql));
    } catch (e) {
      // SQL 错误已由 invoke 封装 toast，这里不再重复提示。
      if (normalizeError(e).code === "cancelled") toast.info("任务已取消");
    } finally {
      setRunning(false);
    }
  };

  const onSqlKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      run();
    }
  };

  return (
    <ToolShell
      title="SQLite 查询器"
      description="扫描或选择本机 SQLite 数据库文件，浏览表结构与行数，直接执行 SQL 查看结果。"
    >
      <div className="mx-auto max-w-5xl space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">数据源</CardTitle>
            <CardDescription>
              扫描应用数据目录中的候选数据库文件，或手动选择任意 SQLite 数据库。
            </CardDescription>
            <CardAction>
              <div className="flex gap-2">
                <Button variant="outline" size="sm" onClick={scan} disabled={scanning}>
                  {scanning ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <RefreshCw className="size-4" />
                  )}
                  扫描应用数据目录
                </Button>
                <Button variant="outline" size="sm" onClick={pickDb}>
                  <FolderOpen className="size-4" />
                  选择 db 文件…
                </Button>
              </div>
            </CardAction>
          </CardHeader>
          <CardContent className="space-y-3">
            {connected && (
              <div className="flex items-center gap-2 rounded-md bg-muted/40 px-3 py-2 text-sm">
                <Badge className="shrink-0">{fileName(connected)}</Badge>
                <span
                  className="min-w-0 flex-1 truncate text-xs text-muted-foreground"
                  title={connected}
                >
                  {connected}
                </span>
                <Button variant="ghost" size="sm" className="shrink-0" onClick={disconnect}>
                  <Unplug className="size-3.5" />
                  断开
                </Button>
              </div>
            )}
            {candidates.length > 0 && (
              <div className="max-h-72 space-y-1 overflow-y-auto">
                {candidates.map((p) => (
                  <button
                    key={p}
                    type="button"
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted/60",
                      connected === p && "bg-muted"
                    )}
                    title={p}
                    onClick={() => connect(p)}
                  >
                    <Database className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1 truncate">{fileName(p)}</span>
                  </button>
                ))}
              </div>
            )}
            {candidates.length === 0 && !connected && (
              <p className="text-sm text-muted-foreground">
                {scanning
                  ? "扫描中…"
                  : "尚无扫描结果，可点击「扫描应用数据目录」或手动选择 db 文件。"}
              </p>
            )}
          </CardContent>
        </Card>

        {connected && (
          <Card>
            <CardHeader>
              <CardTitle className="text-base">
                数据表
                <span className="ml-2 text-sm font-normal text-muted-foreground">
                  共 {tables.length} 张
                </span>
              </CardTitle>
              <CardDescription>点击表名把 SELECT 查询填入下方 SQL 输入框。</CardDescription>
            </CardHeader>
            <CardContent className="max-h-72 space-y-1 overflow-y-auto">
              {tablesLoading ? (
                <div className="flex justify-center py-6 text-muted-foreground">
                  <Loader2 className="size-5 animate-spin" />
                </div>
              ) : tables.length === 0 ? (
                <p className="text-sm text-muted-foreground">该数据库没有用户表。</p>
              ) : (
                tables.map((t) => (
                  <button
                    key={t.name}
                    type="button"
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted/60"
                    onClick={() => setSql(`SELECT * FROM ${quoteIdent(t.name)} LIMIT 100`)}
                  >
                    <Table2 className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1 truncate font-mono" title={t.name}>
                      {t.name}
                    </span>
                    <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                      {t.row_count < 0 ? "—" : `${t.row_count} 行`}
                    </span>
                  </button>
                ))
              )}
            </CardContent>
          </Card>
        )}

        <Card>
          <CardHeader>
            <CardTitle className="text-base">SQL 查询</CardTitle>
            <CardDescription>
              查询语句（SELECT / WITH 等）返回结果表格，其他语句直接执行并显示影响行数。
            </CardDescription>
            <CardAction>
              <Button size="sm" onClick={run} disabled={running}>
                {running ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <Play className="size-4" />
                )}
                运行
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="space-y-3">
            <textarea
              className="w-full resize-y rounded-lg border border-input bg-transparent px-2.5 py-2 font-mono text-sm outline-none transition-colors placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30"
              rows={6}
              value={sql}
              placeholder={'例如：SELECT * FROM "chapters" LIMIT 100'}
              onChange={(e) => setSql(e.target.value)}
              onKeyDown={onSqlKeyDown}
            />
            {result &&
              (result.rows_affected != null ? (
                <p className="text-sm text-muted-foreground">
                  执行成功，影响 {result.rows_affected} 行
                </p>
              ) : (
                <div className="space-y-2">
                  {result.truncated && (
                    <p className="text-xs text-amber-600 dark:text-amber-400">
                      结果已截断，仅显示前 {result.total_rows} 行
                    </p>
                  )}
                  {result.rows.length === 0 ? (
                    <p className="text-sm text-muted-foreground">查询成功，0 行</p>
                  ) : (
                    <div className="max-h-96 overflow-auto rounded-lg border">
                      <table className="w-full text-sm">
                        <thead className="sticky top-0 z-10 bg-muted">
                          <tr>
                            {result.columns.map((c, j) => (
                              <th
                                key={`col-${j}`}
                                className="whitespace-nowrap px-2.5 py-1.5 text-left font-medium"
                                title={c}
                              >
                                {c}
                              </th>
                            ))}
                          </tr>
                        </thead>
                        <tbody>
                          {result.rows.map((row, i) => (
                            <tr key={i} className="border-t">
                              {row.map((cell, j) => (
                                <td
                                  key={j}
                                  className="max-w-64 truncate px-2.5 py-1.5"
                                  title={cell ?? "NULL"}
                                >
                                  {cell ?? <span className="text-muted-foreground">NULL</span>}
                                </td>
                              ))}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                </div>
              ))}
          </CardContent>
        </Card>
      </div>
    </ToolShell>
  );
}
