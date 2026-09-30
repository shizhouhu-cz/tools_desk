import { useState } from "react";
import { NavLink } from "react-router-dom";
import { PanelLeftClose, PanelLeftOpen, Settings, Wrench } from "lucide-react";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { TOOLS } from "@/tools/registry";

const COLLAPSED_KEY = "sidebar-collapsed";

export function Sidebar() {
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(COLLAPSED_KEY) === "1",
  );

  const toggle = () =>
    setCollapsed((c) => {
      localStorage.setItem(COLLAPSED_KEY, c ? "0" : "1");
      return !c;
    });

  const itemClass = ({ isActive }: { isActive: boolean }) =>
    cn(
      "flex items-center rounded-md text-sm transition-colors",
      collapsed ? "justify-center px-0 py-2" : "gap-2.5 px-3 py-2",
      isActive
        ? "bg-sidebar-accent font-medium text-sidebar-accent-foreground"
        : "text-sidebar-foreground/80 hover:bg-sidebar-accent/50",
    );

  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col border-r bg-sidebar transition-[width] duration-200",
        collapsed ? "w-14" : "w-60",
      )}
    >
      <div
        className={cn(
          "flex items-center border-b py-3",
          collapsed ? "flex-col gap-2 px-2" : "gap-2 px-3",
        )}
      >
        <div className="flex size-8 shrink-0 items-center justify-center">
          <Wrench className="size-5 text-primary" />
        </div>
        {!collapsed && (
          <span className="flex-1 font-semibold tracking-tight">工具集</span>
        )}
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={toggle}
          title={collapsed ? "展开导航" : "收起导航"}
        >
          {collapsed ? (
            <PanelLeftOpen className="size-4" />
          ) : (
            <PanelLeftClose className="size-4" />
          )}
        </Button>
      </div>

      <nav className={cn("flex-1 space-y-1 overflow-y-auto", collapsed ? "p-1.5" : "p-2")}>
        {TOOLS.map((tool) => (
          <NavLink
            key={tool.id}
            to={tool.path}
            title={collapsed ? `${tool.title} — ${tool.subtitle}` : undefined}
            className={itemClass}
          >
            <tool.icon className="size-4 shrink-0" />
            {!collapsed && (
              <span className="min-w-0">
                {tool.title}
                <span className="block truncate text-xs font-normal text-muted-foreground">
                  {tool.subtitle}
                </span>
              </span>
            )}
          </NavLink>
        ))}
      </nav>

      <div className={cn("border-t", collapsed ? "p-1.5" : "p-2")}>
        <NavLink to="/settings" title={collapsed ? "设置" : undefined} className={itemClass}>
          <Settings className="size-4 shrink-0" />
          {!collapsed && <span>设置</span>}
        </NavLink>
      </div>
    </aside>
  );
}
