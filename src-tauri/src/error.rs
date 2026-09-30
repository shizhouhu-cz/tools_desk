use serde::{ser::Serializer, Serialize};

/// 应用统一错误类型。序列化为 `{code, message}`，前端按 code 区分处理。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Internal(String),
    #[error("任务已取消")]
    Cancelled,
    #[error("转换失败: {0}")]
    ConversionFailed(String),
    #[error("渲染失败: {0}")]
    RenderingFailed(String),
    #[error("水印处理失败: {0}")]
    WatermarkFailed(String),
    #[error("参数错误: {0}")]
    BadRequest(String),
    #[error("配置读写失败: {0}")]
    Config(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Internal(_) => "internal",
            AppError::Cancelled => "cancelled",
            AppError::ConversionFailed(_) => "conversion_failed",
            AppError::RenderingFailed(_) => "rendering_failed",
            AppError::WatermarkFailed(_) => "watermark_failed",
            AppError::BadRequest(_) => "bad_request",
            AppError::Config(_) => "config",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Payload<'a> {
            code: &'a str,
            message: String,
        }
        Payload {
            code: self.code(),
            message: self.to_string(),
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
