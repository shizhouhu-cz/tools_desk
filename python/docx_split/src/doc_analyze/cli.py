"""doc-analyze 命令行：ingest / tree / stats（人工调试）+ extract（Tauri 集成协议）。

extract --json 的 stdout 仅输出 JSONL 协议行（progress/result/error），
人类可读诊断一律走 stderr。
"""
from __future__ import annotations

import json
from pathlib import Path

import typer

from doc_analyze.config import load_settings

app = typer.Typer(
    help="DOCX 结构化拆分（章节树 + 表格 → SQLite）",
    no_args_is_help=True,
)


def _emit(obj: dict) -> None:
    typer.echo(json.dumps(obj, ensure_ascii=False))


@app.command("extract")
def extract(
    path: Path = typer.Argument(..., exists=True, dir_okay=False, readable=True, help="DOCX 文件路径"),
    db: Path = typer.Option(..., "--db", help="SQLite 数据库路径"),
    json_out: bool = typer.Option(False, "--json", help="stdout 输出 JSONL 协议（Tauri 集成用）"),
) -> None:
    """结构化拆分入库；--json 时 stdout 仅输出协议行。"""
    try:
        from doc_analyze.pipeline.ingest import ingest_file
    except ImportError as e:
        if json_out:
            _emit({"type": "error", "message": f"依赖缺失：{e}"})
        else:
            typer.secho(f"依赖缺失：{e}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=3)

    settings = load_settings(db_path=db)

    def on_progress(stage: str, current: int, total: int, message: str | None) -> None:
        if json_out:
            _emit(
                {
                    "type": "progress",
                    "stage": stage,
                    "current": current,
                    "total": total,
                    "message": message or "",
                }
            )

    try:
        stats = ingest_file(path, settings, progress=on_progress)
    except Exception as e:  # noqa: BLE001
        if json_out:
            _emit({"type": "error", "message": str(e)})
        else:
            typer.secho(f"解析失败：{e}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=1)

    if json_out:
        _emit(
            {
                "type": "result",
                "ok": True,
                "doc_id": stats["doc_id"],
                "input": str(path),
                "stats": stats,
            }
        )
    else:
        typer.echo(f"入库完成：{stats}")


@app.command("ingest")
def ingest(
    path: Path = typer.Argument(..., exists=True, dir_okay=False, readable=True, help="DOCX 文件路径"),
) -> None:
    """解析 DOCX 并结构化入库（人读输出，手动调试用）。"""
    try:
        from doc_analyze.pipeline.ingest import ingest_file
    except ImportError as e:
        typer.secho(f"pipeline 未就绪：{e}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=3)
    settings = load_settings()
    stats = ingest_file(path, settings)
    typer.echo(f"入库完成：{stats}")


@app.command("tree")
def tree(
    doc_id: int = typer.Argument(..., help="文档 ID"),
) -> None:
    """打印文档章节树（每级缩进两空格，needs_review 标 *）。"""
    from doc_analyze.db import connect as db_connect

    settings = load_settings()
    conn = db_connect(settings.db_path)
    try:
        rows = conn.execute(
            "SELECT id, level, number_prefix, title, needs_review FROM chapters "
            "WHERE doc_id=? ORDER BY order_num",
            (doc_id,),
        ).fetchall()
    finally:
        conn.close()
    if not rows:
        typer.echo(f"文档 {doc_id} 无章节数据")
        raise typer.Exit(code=1)
    for r in rows:
        indent = "  " * max((r["level"] or 1) - 1, 0)
        prefix = (r["number_prefix"] or "").strip()
        name = f"{prefix} {r['title']}" if prefix else r["title"]
        mark = " *" if r["needs_review"] else ""
        typer.echo(f"{indent}{r['id']} {name}{mark}")


@app.command("stats")
def stats(
    doc_id: int = typer.Argument(..., help="文档 ID"),
) -> None:
    """打印文档统计：状态、章节数、needs_review 数、表格数。"""
    from doc_analyze.db import connect as db_connect

    settings = load_settings()
    conn = db_connect(settings.db_path)
    try:
        doc = conn.execute(
            "SELECT doc_name, status, total_chars, error_msg FROM documents WHERE id=?",
            (doc_id,),
        ).fetchone()
        chapter_count = conn.execute(
            "SELECT COUNT(*) FROM chapters WHERE doc_id=?", (doc_id,)
        ).fetchone()[0]
        review_count = conn.execute(
            "SELECT COUNT(*) FROM chapters WHERE doc_id=? AND needs_review=1", (doc_id,)
        ).fetchone()[0]
        table_count = conn.execute(
            "SELECT COUNT(*) FROM tables WHERE doc_id=?", (doc_id,)
        ).fetchone()[0]
    finally:
        conn.close()
    if doc is None:
        typer.secho(f"文档 {doc_id} 不存在", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=1)
    info = {
        "doc_id": doc_id,
        "doc_name": doc["doc_name"],
        "status": doc["status"],
        "total_chars": doc["total_chars"],
        "error_msg": doc["error_msg"],
        "chapters": chapter_count,
        "needs_review": review_count,
        "tables": table_count,
    }
    typer.echo(str(info))


if __name__ == "__main__":
    app()
