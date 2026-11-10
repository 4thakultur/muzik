//! Extractor wrappers whose rejections use the JSON error format.

use axum::extract::{
    rejection::{JsonRejection, PathRejection, QueryRejection},
    FromRequest, FromRequestParts,
};

use crate::error::AppError;

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
pub struct Query<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(AppError))]
pub struct Path<T>(pub T);

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct Json<T>(pub T);

impl From<QueryRejection> for AppError {
    fn from(r: QueryRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}

impl From<PathRejection> for AppError {
    fn from(r: PathRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}

impl From<JsonRejection> for AppError {
    fn from(r: JsonRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}
