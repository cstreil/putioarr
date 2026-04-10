use crate::{
    http::handlers::{handle_torrent_add, handle_torrent_get, handle_torrent_remove},
    services::transmission::{TransmissionConfig, TransmissionRequest, TransmissionResponse},
    AppData,
};
use actix_web::{
    get,
    http::header::{ContentType, Header},
    post, web, HttpRequest, HttpResponse,
};
use actix_web_httpauth::headers::authorization::{Authorization, Basic};
use anyhow::{bail, Context, Result};
use log::error;
use serde_json::json;

const SESSION_ID: &str = "useless-session-id";

#[post("/transmission/rpc")]
pub(crate) async fn rpc_post(
    payload: web::Json<TransmissionRequest>,
    req: HttpRequest,
    app_data: web::Data<AppData>,
) -> HttpResponse {
    rpc_post_impl(payload, req, app_data, None).await
}

#[post("/{app}/transmission/rpc")]
pub(crate) async fn rpc_post_app(
    path: web::Path<String>,
    payload: web::Json<TransmissionRequest>,
    req: HttpRequest,
    app_data: web::Data<AppData>,
) -> HttpResponse {
    let app = path.into_inner();
    let category = crate::http::handlers::app_to_category(&app);
    rpc_post_impl(payload, req, app_data, Some(category)).await
}

async fn rpc_post_impl(
    payload: web::Json<TransmissionRequest>,
    req: HttpRequest,
    app_data: web::Data<AppData>,
    category: Option<&str>,
) -> HttpResponse {
    // Not sure if necessary since we might just look at the session id.
    if validate_user(req, &app_data).await.is_err() {
        return HttpResponse::Conflict()
            .content_type(ContentType::json())
            .insert_header(("X-Transmission-Session-Id", SESSION_ID))
            .body("");
    }

    let arguments = match payload.method.as_str() {
        "session-get" => Some(json!(TransmissionConfig {
            download_dir: app_data.config.download_directory.clone(),
            ..Default::default()
        })),
        "torrent-get" => match handle_torrent_get(&app_data, category).await {
            Ok(v) => v,
            Err(e) => {
                error!("{}", e);
                return HttpResponse::InternalServerError().body(e.to_string());
            }
        },
        "torrent-set" => None, // Nothing to do here
        "queue-move-top" => None,
        "torrent-remove" => match handle_torrent_remove(&payload, &app_data).await {
            Ok(v) => v,
            Err(e) => {
                error!("{}", e);
                return HttpResponse::InternalServerError().body(e.to_string());
            }
        },
        "torrent-add" => match handle_torrent_add(&payload, &app_data, category).await {
            Ok(v) => v,
            Err(e) => {
                error!("{}", e);
                return HttpResponse::BadRequest().body(e.to_string());
            }
        },
        _ => {
            error!("Unknown Transmission RPC method: {}", payload.method);
            return HttpResponse::BadRequest()
                .content_type(ContentType::json())
                .json(json!({
                    "result": "error",
                    "error": format!("Unknown method: {}", payload.method)
                }));
        }
    };

    let response = TransmissionResponse {
        result: String::from("success"),
        arguments,
    };

    HttpResponse::Ok()
        .content_type(ContentType::json())
        .json(response)
}

/// Pretty much only used for authentication.
#[get("/transmission/rpc")]
async fn rpc_get(req: HttpRequest, app_data: web::Data<AppData>) -> HttpResponse {
    if validate_user(req, &app_data).await.is_err() {
        return HttpResponse::Forbidden().body("forbidden");
    }

    HttpResponse::Conflict()
        .content_type(ContentType::json())
        .insert_header(("X-Transmission-Session-Id", SESSION_ID))
        .body("")
}

#[get("/{app}/transmission/rpc")]
async fn rpc_get_app(req: HttpRequest, app_data: web::Data<AppData>) -> HttpResponse {
    if validate_user(req, &app_data).await.is_err() {
        return HttpResponse::Forbidden().body("forbidden");
    }

    HttpResponse::Conflict()
        .content_type(ContentType::json())
        .insert_header(("X-Transmission-Session-Id", SESSION_ID))
        .body("")
}

async fn validate_user(req: HttpRequest, app_data: &web::Data<AppData>) -> Result<()> {
    let auth = Authorization::<Basic>::parse(&req)?;
    let user_username = auth.as_ref().user_id();
    let user_password = auth.as_ref().password().context("No password given")?;
    if user_username == app_data.config.username && user_password == app_data.config.password {
        Ok(())
    } else {
        bail!("Username or password mismatch")
    }
}
