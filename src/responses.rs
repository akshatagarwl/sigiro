//! Typed response structs for API endpoints.
//!
//! Replaces `serde_json::Value` return types with compile-time verified structs,
//! enabling accurate OpenAPI schema generation via utoipa.

use serde::Serialize;
use std::collections::HashMap;
use utoipa::ToSchema;

/// GET /v1/services
#[derive(Serialize, ToSchema)]
pub struct ServicesResponse {
    pub services: Vec<ServiceEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct ServiceEntry {
    pub name: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub span_count: i64,
}

/// POST /admin/keys
#[derive(Serialize, ToSchema)]
pub struct CreateKeyResponse {
    pub key: String,
}

/// DELETE /admin/keys/{short_token}
#[derive(Serialize, ToSchema)]
pub struct RevokeKeyResponse {
    pub revoked: bool,
    pub short_token: String,
}

/// GET /admin/status
#[derive(Serialize, ToSchema)]
pub struct AdminStatusResponse {
    #[schema(value_type = HashMap<String, String>)]
    pub status: HashMap<String, String>,
}

/// POST /v1/federation/workers
#[derive(Serialize, ToSchema)]
pub struct RegisterWorkerResponse {
    pub registered: bool,
    pub id: String,
}

/// DELETE /v1/federation/workers/{id}
#[derive(Serialize, ToSchema)]
pub struct DeregisterWorkerResponse {
    pub deregistered: bool,
}

/// GET /v1/federation/workers
#[derive(Serialize, ToSchema)]
pub struct ListWorkersResponse {
    pub workers: Vec<crate::federation::WorkerInfo>,
}
