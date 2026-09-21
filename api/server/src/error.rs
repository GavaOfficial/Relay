use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub enum AppError {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest(&'static str),
    Conflict(&'static str),
    Internal(String),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound
        } else {
            AppError::Internal(e.to_string())
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m).into_response(),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m).into_response(),
            AppError::Internal(m) => {
                tracing::error!("internal error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}
