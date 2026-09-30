"""LLM Provider 层：mock / OpenAI 兼容（httpx 裸调，无 langchain 依赖）。

- 结构恢复走 complete_json（JSON 输出、temperature=0）
- 未配置 API Key 或调用失败时上层必须走降级路径（design.md 4.4）
"""
from __future__ import annotations

import json
import re
import sys
import time
from typing import List

import httpx

from doc_analyze.config import Settings

_JSON_FENCE = re.compile(r"```(?:json)?\s*(.*?)\s*```", re.DOTALL)


class LLMError(RuntimeError):
    pass


def _extract_json(text: str) -> dict:
    """从模型输出中提取 JSON 对象（容忍 markdown 代码栅栏）。"""
    m = _JSON_FENCE.search(text)
    raw = m.group(1) if m else text
    start, end = raw.find("{"), raw.rfind("}")
    if start < 0 or end <= start:
        raise LLMError(f"模型输出不含 JSON：{text[:200]}")
    try:
        return json.loads(raw[start : end + 1])
    except json.JSONDecodeError as e:
        raise LLMError(f"JSON 解析失败：{e}") from e


class MockProvider:
    """确定性 mock：结构恢复回显建议层级（无网络依赖，用于离线/测试）。"""

    def complete_json(self, system: str, user: str) -> dict:
        payload = json.loads(user)
        items: List[dict] = payload["items"]
        out = []
        for it in items:
            sug = it.get("suggested_level")
            if isinstance(sug, int) and sug >= 1:
                # 锚点：回显建议层级
                out.append({"idx": it["idx"], "is_title": True, "level": sug})
            else:
                # 待定项：mock 保守策略——不升格为标题（留给真实模型裁决）
                out.append({"idx": it["idx"], "is_title": False, "level": None})
        return {"items": out}


class OpenAICompatProvider:
    """OpenAI 兼容接口（ChatGPT、DashScope 兼容模式、内网网关均可用）。

    complete_json：temperature=0 + 客户端 JSON 抽取与重试（不传
    response_format，与原 langchain 实现行为一致，兼容严格网关）。
    鉴权二选一：OPENAI_API_KEY（默认 Bearer 头）或 openai_auth_header
    （完整 Authorization 值，用于 anvil 等要求裸 token 的网关）。
    """

    def __init__(self, settings: Settings):
        if not settings.openai_api_key and not settings.openai_auth_header:
            raise LLMError("OPENAI_API_KEY 或 DOC_ANALYZE_OPENAI_AUTH_HEADER 至少配置一个")
        self._settings = settings

    def _headers(self) -> dict:
        auth = self._settings.openai_auth_header or (
            f"Bearer {self._settings.openai_api_key or 'unused'}"
        )
        return {"Authorization": auth, "Content-Type": "application/json"}

    def complete_json(self, system: str, user: str) -> dict:
        base = (self._settings.openai_base_url or "https://api.openai.com/v1").rstrip("/")
        # 容错：base_url 已包含 /chat/completions 时不再重复拼接
        url = base if base.endswith("/chat/completions") else f"{base}/chat/completions"
        payload = {
            "model": self._settings.recovery_model,
            "temperature": 0,
            # 关闭 Qwen3 系思考模式（thinking 是耗时主因）。
            # 注意：必须放 JSON body 顶层（DashScope 兼容），extra_body 是 OpenAI
            # SDK 的客户端参数，httpx 裸调时无效；vLLM 自部署用 chat_template_kwargs。
            "enable_thinking": False,
            "chat_template_kwargs": {"enable_thinking": False},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        }
        last_err: Exception | None = None
        print(f"[llm] POST {url} model={payload['model']}", file=sys.stderr)
        with httpx.Client(timeout=httpx.Timeout(180.0)) as client:
            for attempt in range(2):  # 1 次重试（design.md 4.4）
                t0 = time.monotonic()
                try:
                    resp = client.post(url, json=payload, headers=self._headers())
                    resp.raise_for_status()
                    print(
                        f"[llm] HTTP {resp.status_code} 耗时 {time.monotonic() - t0:.1f}s",
                        file=sys.stderr,
                    )
                    content = resp.json()["choices"][0]["message"]["content"]
                    return _extract_json(content)
                except Exception as e:  # noqa: BLE001
                    print(
                        f"[llm] 第{attempt + 1}次调用失败(已耗时 {time.monotonic() - t0:.1f}s): {e}",
                        file=sys.stderr,
                    )
                    last_err = e
        raise LLMError(f"complete_json 调用失败：{last_err}")


def get_provider(settings: Settings):
    """按配置返回 Provider；未配置/凭证缺失/未知均返回 None（上层走降级，不抛异常）。"""
    if settings.llm_provider == "mock":
        return MockProvider()
    if settings.llm_provider == "openai":
        if not settings.openai_api_key and not settings.openai_auth_header:
            print(
                "[llm] provider=openai 但未配置凭证（OPENAI_API_KEY / DOC_ANALYZE_OPENAI_AUTH_HEADER），降级规则模式",
                file=sys.stderr,
            )
            return None
        return OpenAICompatProvider(settings)
    return None
