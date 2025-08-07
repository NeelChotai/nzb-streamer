use std::{error::Error, string::FromUtf8Error};

use axum::{
    Json,
    extract::multipart::MultipartError,
    response::{IntoResponse, Response},
};
use http::StatusCode;
use serde_json::json;
use thiserror::Error;
use tracing::error;

use crate::{
    archive::error::Par2Error, nntp::error::NntpError, nzb::error::NzbError,
    scheduler::error::SchedulerError,
};

#[derive(Debug, Error)]
pub enum RestError {
    #[error("Error encountered reading upload as NZB file")]
    NzbParse(#[from] NzbError),

    #[error("Error encountered during file upload")]
    MultiPart(#[from] MultipartError),

    #[error("Please upload an NZB file using the 'nzb' form field")]
    MissingNzb,

    #[error("Error encountered reading uploaded file contents")]
    Utf8Parse(#[from] FromUtf8Error),

    #[error("Error encountered attempting to decode PAR2 recovery file")]
    Par2(#[from] Par2Error),

    #[error("Invalid range header")]
    InvalidRange,

    #[error("Requested range not satisfiable")]
    RangeNotSatisfiable,

    #[error("Session not found")]
    SessionNotFound,

    #[error("Error in NNTP client")]
    Nntp(#[from] NntpError),

    #[error("Error in background downloads")]
    BackgroundDownload(#[from] tokio::task::JoinError),

    #[error("Error in scheduler")]
    Scheduler(#[from] SchedulerError),
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        error!("{}: {:?}", self, self.source());

        let status = match self {
            RestError::NzbParse(_) => StatusCode::BAD_REQUEST,
            RestError::MultiPart(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RestError::MissingNzb => StatusCode::BAD_REQUEST,
            RestError::Utf8Parse(_) => StatusCode::BAD_REQUEST,
            RestError::Par2(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RestError::InvalidRange => StatusCode::BAD_REQUEST,
            RestError::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            RestError::SessionNotFound => StatusCode::NOT_FOUND,
            RestError::Nntp(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RestError::BackgroundDownload(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RestError::Scheduler(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let payload = Json(json!({"message": self.to_string()}));

        (status, payload).into_response()
    }
}
