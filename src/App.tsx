import { HashRouter, Navigate, useLocation } from "react-router-dom";
import { Sidebar } from "@/components/layout/Sidebar";
import { Toaster } from "@/components/ui/sonner";
import { TOOLS } from "@/tools/registry";
import Settings from "@/tools/settings";

/**
 * 所有工具页常驻挂载，仅切换可见性（keep-alive）：
 * 切换导航不丢失表单内容与运行中任务的状态/进度。
 */
function Workspace() {
  const { pathname } = useLocation();
  const known = pathname === "/settings" || TOOLS.some((t) => t.path === pathname);
  if (!known) return <Navigate to={TOOLS[0].path} replace />;
  return (
    <>
      {TOOLS.map((tool) => (
        <div key={tool.id} className={pathname === tool.path ? "contents" : "hidden"}>
          {tool.element}
        </div>
      ))}
      <div className={pathname === "/settings" ? "contents" : "hidden"}>
        <Settings />
      </div>
    </>
  );
}

export default function App() {
  return (
    <HashRouter>
      <div className="flex h-screen overflow-hidden">
        <Sidebar />
        <main className="min-w-0 flex-1">
          <Workspace />
        </main>
      </div>
      <Toaster position="top-center" richColors />
    </HashRouter>
  );
}
