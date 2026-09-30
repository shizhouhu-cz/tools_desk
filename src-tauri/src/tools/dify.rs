use std::time::Duration;

use serde_json::Value;

use crate::error::{AppError, AppResult};

/// Dify 工作流客户端：目录命名工作流（入参 file_name/prompt，输出目录名）。
#[derive(Clone)]
pub struct DifyClient {
    http: reqwest::Client,
    /// 规范化后的 API 根地址，形如 `https://host/v1`（无尾斜杠）。
    base_url: String,
    api_key: String,
}

impl DifyClient {
    pub fn new(base_url: &str, api_key: &str, timeout: Duration) -> AppResult<Self> {
        let key = api_key.trim().to_string();
        if key.is_empty() {
            return Err(AppError::BadRequest("API Key 不能为空".into()));
        }
        let mut base = base_url.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err(AppError::BadRequest("服务地址不能为空".into()));
        }
        if !base.starts_with("http://") && !base.starts_with("https://") {
            base = format!("https://{base}");
        }
        // 用户可能只填到域名或带 /v1，统一补齐。
        if !base.ends_with("/v1") {
            base.push_str("/v1");
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| AppError::Internal(format!("初始化 HTTP 客户端失败: {e}")))?;
        Ok(Self {
            http,
            base_url: base,
            api_key: key,
        })
    }

    /// 探测接口连通性（GET /parameters）。返回 (是否通畅, 说明)。
    pub async fn probe(&self) -> (bool, String) {
        let url = format!("{}/parameters", self.base_url);
        match self.http.get(&url).bearer_auth(&self.api_key).send().await {
            Ok(resp) => match resp.status().as_u16() {
                200 => (true, "连接成功，工作流接口可访问".to_string()),
                401 | 403 => (
                    false,
                    format!(
                        "认证失败（HTTP {}），请检查 API Key 是否为该工作流的密钥",
                        resp.status().as_u16()
                    ),
                ),
                code => (false, format!("服务返回异常状态: HTTP {code}")),
            },
            Err(e) => (false, format!("无法连接服务: {e}")),
        }
    }

    /// 调用工作流（blocking 模式），提取目录名。
    /// 入参：input_file_name（文件名）、rule_prompt（用户提示词）；
    /// 出参：优先取结构化输出的 clean_text 属性，回退取 outputs 中第一个字符串值。
    pub async fn resolve_dir_name(&self, file_name: &str, prompt: &str) -> AppResult<String> {
        let url = format!("{}/workflows/run", self.base_url);
        let mut inputs = serde_json::Map::new();
        inputs.insert(
            "input_file_name".into(),
            Value::String(file_name.to_string()),
        );
        inputs.insert(
            "rule_prompt".into(),
            Value::String(prompt.trim().to_string()),
        );
        let body = serde_json::json!({
            "inputs": inputs,
            "response_mode": "blocking",
            "user": "tools-app",
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("请求 Dify 工作流失败: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Internal(format!("读取 Dify 响应失败: {e}")))?;
        if !status.is_success() {
            return Err(AppError::Internal(format!(
                "Dify 工作流返回 HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 200)
            )));
        }

        let v: Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Internal(format!("解析 Dify 响应失败: {e}")))?;
        if let Some(st) = v.pointer("/data/status").and_then(Value::as_str) {
            if st != "succeeded" {
                let err = v
                    .pointer("/data/error")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误");
                return Err(AppError::Internal(format!("工作流执行失败（{st}）: {err}")));
            }
        }
        extract_dir_name(&v)
    }
}

/// 从 blocking 响应中提取目录名：
/// 1) `data.outputs.clean_text` 为字符串；
/// 2) `data.outputs` 某个值为对象（结构化输出），其 `clean_text` 为字符串；
/// 3) 回退：outputs 中第一个字符串值。
fn extract_dir_name(v: &Value) -> AppResult<String> {
    let outputs = v
        .pointer("/data/outputs")
        .ok_or_else(|| AppError::Internal("Dify 响应缺少 data.outputs".into()))?;
    let Some(map) = outputs.as_object() else {
        return Err(AppError::Internal("data.outputs 不是对象".into()));
    };
    if let Some(s) = map.get("clean_text").and_then(Value::as_str) {
        return Ok(s.to_string());
    }
    for val in map.values() {
        if let Some(inner) = val.as_object() {
            if let Some(s) = inner.get("clean_text").and_then(Value::as_str) {
                return Ok(s.to_string());
            }
        }
    }
    map.values()
        .find_map(|val| val.as_str().map(str::to_string))
        .ok_or_else(|| {
            AppError::Internal("工作流输出中未找到 clean_text 或字符串类型的目录名".into())
        })
}

/// 清洗目录名：替换 Windows 非法字符、合并空白、去首尾空白/点、限长。
pub fn sanitize_dir_name(raw: &str) -> Option<String> {
    let mapped: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if (c as u32) < 0x20 => ' ',
            c => c,
        })
        .collect();
    let mut s = mapped;
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    let s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        return None;
    }
    Some(s.chars().take(80).collect())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_invalid_chars() {
        assert_eq!(
            sanitize_dir_name("  报告: v1/a?  ").as_deref(),
            Some("报告 v1 a")
        );
        assert_eq!(sanitize_dir_name("..."), None);
        assert_eq!(sanitize_dir_name("  "), None);
        assert_eq!(
            sanitize_dir_name("ok-name_2026").as_deref(),
            Some("ok-name_2026")
        );
    }

    #[test]
    fn base_url_normalized() {
        // 仅验证构造不报错与关键字段（通过公开行为间接测试：探测失败但 URL 规范化不 panic）。
        let c = DifyClient::new("dify.example.com/", "app-123", Duration::from_secs(5));
        assert!(c.is_ok());
        assert!(DifyClient::new("", "app-123", Duration::from_secs(5)).is_err());
        assert!(DifyClient::new("https://x.com", "  ", Duration::from_secs(5)).is_err());
    }

    #[test]
    fn extract_prefers_clean_text() {
        // 平铺：outputs.clean_text 直接是字符串。
        let flat = serde_json::json!({"data": {"outputs": {"clean_text": " 平铺目录 "}}});
        assert_eq!(extract_dir_name(&flat).unwrap(), " 平铺目录 ");

        // 结构化输出：clean_text 嵌套在对象里。
        let nested = serde_json::json!({
            "data": {"outputs": {"output": {"clean_text": "嵌套目录", "other": 1}}}
        });
        assert_eq!(extract_dir_name(&nested).unwrap(), "嵌套目录");

        // 回退：没有 clean_text 时取第一个字符串值。
        let fallback = serde_json::json!({"data": {"outputs": {"result": "回退目录"}}});
        assert_eq!(extract_dir_name(&fallback).unwrap(), "回退目录");

        // 都没有时才报错。
        let empty = serde_json::json!({"data": {"outputs": {}}});
        assert!(extract_dir_name(&empty).is_err());
    }
}
