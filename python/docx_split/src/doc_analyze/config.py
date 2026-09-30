"""全局配置：仅环境变量（打包内嵌运行场景，无 config.toml）。

优先级：显式参数 > 环境变量 > 代码默认值。
"""
from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Settings:
    db_path: Path
    llm_provider: str = "openai"  # mock | openai
    openai_api_key: str | None = None
    openai_base_url: str | None = None
    # 完整 Authorization 头值（内网网关等非 Bearer 鉴权时使用，覆盖默认 "Bearer <key>"）
    openai_auth_header: str | None = None
    recovery_model: str = "qwen-plus"


def _env(key: str) -> str | None:
    v = os.environ.get(key)
    return v.strip() or None if v else None


def load_settings(db_path: str | Path | None = None) -> Settings:
    """构建配置。db_path 优先级：显式参数 > env DOC_ANALYZE_DB > cwd 相对默认值。"""
    db = db_path or _env("DOC_ANALYZE_DB") or "docx_split.db"
    return Settings(
        db_path=Path(db).expanduser(),
        llm_provider=_env("DOC_ANALYZE_LLM_PROVIDER") or "openai",
        openai_api_key=_env("OPENAI_API_KEY"),
        openai_base_url=_env("OPENAI_BASE_URL"),
        openai_auth_header=_env("DOC_ANALYZE_OPENAI_AUTH_HEADER"),
        recovery_model=_env("DOC_ANALYZE_RECOVERY_MODEL") or "qwen-plus",
    )
