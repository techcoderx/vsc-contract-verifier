use actix_web::{ get, post, web, HttpRequest, HttpResponse, Responder };
use actix_multipart::form::{ tempfile::TempFile, MultipartForm, text::Text };
use mongodb::bson::doc;
use tokio_postgres::types::Type;
use serde::{ Serialize, Deserialize };
use serde_json::{ json, Number, Value };
use semver::VersionReq;
use chrono::{ NaiveDateTime, Utc, Duration };
use hex;
use sha2::{ Sha256, Digest };
use jsonwebtoken::{ Header, EncodingKey, DecodingKey, Algorithm, Validation, errors::ErrorKind };
use log::{ error, debug };
use std::io::Read;
use crate::{ config::config, constants::*, types::{ server::{ Context, RespErr }, hive::{ JsonRpcResp, DgpAtBlock } } };

#[get("")]
async fn hello() -> impl Responder {
  HttpResponse::Ok().body("Hello world!")
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
  user: String,
  app: String,
  network: String,
  iat: i64, // Issued at (timestamp)
  exp: i64, // Expiration time (timestamp)
}

fn verify_auth_token(req: &HttpRequest) -> Result<String, RespErr> {
  if config.auth.enabled {
    if let Some(auth_header) = req.clone().headers().get("Authorization") {
      let auth_value = auth_header.to_str().unwrap_or("");
      let parts = auth_value.split(" ").collect::<Vec<&str>>();
      debug!("Authentication header: {}", auth_value);
      if parts.len() < 2 || parts[0] != "Bearer" {
        return Err(RespErr::TokenMissing);
      }
      let mut validation = Validation::new(Algorithm::HS256);
      validation.validate_exp = true;
      validation.leeway = 0;
      let claims = (match
        jsonwebtoken::decode::<Claims>(
          parts[1],
          &DecodingKey::from_secret(hex::decode(config.auth.key.clone().unwrap()).unwrap().as_slice()),
          &validation
        )
      {
        Ok(token_data) => {
          // Additional manual checks if needed
          let now = Utc::now().timestamp();

          // Verify iat is in the past
          if token_data.claims.iat > now {
            return Err(RespErr::TokenExpired);
          }

          Ok(token_data.claims)
        }
        Err(err) =>
          match err.kind() {
            ErrorKind::ExpiredSignature => Err(RespErr::TokenExpired),
            _ => Err(RespErr::TokenInvalid),
          }
      })?;
      return Ok(claims.user);
    } else {
      return Err(RespErr::TokenMissing);
    }
  }
  Ok(String::from(""))
}

#[post("/login")]
async fn login(payload: String, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  if !config.auth.enabled {
    return Ok(HttpResponse::NotFound().json(json!({"error": "Auth is disabled"})));
  }
  let parts: Vec<&str> = payload.split(":").collect();
  if parts.len() != 6 || parts[1] != &config.auth.id.clone().unwrap() || parts[2] != "hive" {
    return Err(RespErr::BadRequest { msg: String::from("Invalid auth message format") });
  }
  let block_num = parts[3].parse::<u64>();
  if block_num.is_err() {
    return Err(RespErr::BadRequest { msg: String::from("Could not parse block number") });
  }
  let block_num = block_num.unwrap();
  let original = (&parts[0..5]).join(":");
  let mut hasher = Sha256::new();
  hasher.update(&original);
  let hash = hex::encode(&hasher.finalize()[..]);
  let verify_req = ctx.http_client
    .post(config.hive_rpc.clone())
    .json::<Value>(
      &json!({
    "id": 1,
    "jsonrpc": "2.0",
    "method": "database_api.verify_signatures",
    "params": {
      "hash": &hash,
      "signatures": [parts[5]],
      "required_owner": [],
      "required_active": [],
      "required_posting": [parts[0]],
      "required_other": []
  }
  })
    )
    .send().await
    .map_err(|_| RespErr::SigVerifyReqFail)?
    .json::<JsonRpcResp>().await
    .map_err(|_| RespErr::SigVerifyReqFail)?;
  let is_valid =
    !verify_req.error.is_some() && verify_req.result.is_some() && verify_req.result.unwrap().clone()["valid"].as_bool().unwrap();
  if !is_valid {
    return Err(RespErr::SigVerifyFail);
  }
  let head_block_num = ctx.http_client
    .get(config.hive_rpc.clone() + "/hafah-api/headblock")
    .send().await
    .map_err(|_| RespErr::SigRecentBlkReqFail)?
    .json::<Number>().await
    .map_err(|_| RespErr::SigRecentBlkReqFail)?;
  if head_block_num.as_u64().unwrap() > block_num + config.auth.timeout_blocks.unwrap_or(20) {
    return Err(RespErr::SigTooOld);
  }
  let dgp_at_block = ctx.http_client
    .get(config.hive_rpc.clone() + "/hafah-api/global-state?block-num=" + &block_num.to_string())
    .send().await
    .map_err(|_| RespErr::SigRecentBlkReqFail)?
    .json::<DgpAtBlock>().await
    .map_err(|_| RespErr::SigRecentBlkReqFail)?;
  if &dgp_at_block.hash != parts[4] {
    return Err(RespErr::SigBhNotMatch);
  }

  // generate jwt
  let now = Utc::now();
  let iat = now.timestamp();
  let exp = (now + Duration::hours(1)).timestamp();
  let claims = Claims {
    user: String::from(parts[0]),
    app: config.auth.id.clone().unwrap(),
    network: String::from("hive"),
    iat,
    exp,
  };
  let decoded_secret = hex::decode(config.auth.key.clone().unwrap()).map_err(|_| RespErr::TokenGenFail)?;
  let token = jsonwebtoken
    ::encode(&Header::default(), &claims, &EncodingKey::from_secret(&decoded_secret))
    .map_err(|_| RespErr::TokenGenFail)?;
  Ok(HttpResponse::Ok().json(json!({ "access_token": token })))
}

#[derive(Serialize, Deserialize)]
struct ReqVerifyNew {
  license: String,
  lang: String,
  dependencies: Value,
}

#[post("/verify/{address}/new")]
async fn verify_new(
  req: HttpRequest,
  path: web::Path<String>,
  req_data: web::Json<ReqVerifyNew>,
  ctx: web::Data<Context>
) -> Result<HttpResponse, RespErr> {
  let username = verify_auth_token(&req)?;
  let address = path.into_inner();
  let contract = ctx.vsc_db.contracts.find_one(doc! { "id": &address }).await.map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if contract.is_none() {
    return Ok(HttpResponse::NotFound().json(json!({"error": "contract not found"})));
  }
  let contract = contract.unwrap();
  let can_verify: String = ctx.db
    .query(
      "SELECT vsc_cv.can_verify_new($1,$2,$3);",
      &[
        (&address, Type::VARCHAR),
        (&req_data.license, Type::VARCHAR),
        (&req_data.lang, Type::VARCHAR),
      ]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?
    [0].get(0);
  if can_verify.len() > 0 {
    return Err(RespErr::BadRequest { msg: can_verify });
  }
  // check required dependencies
  match req_data.lang.as_str() {
    "assemblyscript" => {
      if !req_data.dependencies.is_object() {
        return Err(RespErr::BadRequest { msg: String::from("Dependencies must be an object") });
      }
      let test_utils = req_data.dependencies.get(ASC_TEST_UTILS_NAME);
      let sdk = req_data.dependencies.get(ASC_SDK_NAME);
      let assemblyscript = req_data.dependencies.get(ASC_NAME);
      let assemblyscript_json = req_data.dependencies.get(ASC_JSON_NAME);
      if test_utils.is_none() || sdk.is_none() || assemblyscript.is_none() || assemblyscript_json.is_none() {
        return Err(RespErr::BadRequest {
          msg: format!(
            "The following dependencies are required: {}, {}, {}, {}",
            ASC_TEST_UTILS_NAME,
            ASC_SDK_NAME,
            ASC_NAME,
            ASC_JSON_NAME
          ),
        });
      }
      if let Value::Object(map) = &req_data.dependencies {
        // Iterate over the keys and values in the map
        for (key, val) in map.iter() {
          if !val.is_string() {
            return Err(RespErr::BadRequest { msg: String::from("Dependency versions must be strings") });
          }
          VersionReq::parse(val.as_str().unwrap()).map_err(|e| RespErr::BadRequest {
            msg: format!("Invalid semver for dependency {}: {}", key, e.to_string()),
          })?;
        }
      }
    }
    _ => {
      return Err(RespErr::BadRequest { msg: String::from("Language is currently unsupported") });
    }
  }
  // clear already uploaded source codes when the previous ones failed verification
  ctx.db
    .query("DELETE FROM vsc_cv.source_code WHERE contract_addr=$1;", &[(&address, Type::VARCHAR)]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  ctx.db
    .query(
      "SELECT vsc_cv.verify_new($1,$2,$3,$4,$5,$6,$7);",
      &[
        (&address, Type::VARCHAR),
        (&contract.code, Type::VARCHAR),
        (&username, Type::VARCHAR),
        (&Utc::now().naive_utc(), Type::TIMESTAMP),
        (&req_data.license, Type::VARCHAR),
        (&req_data.lang, Type::VARCHAR),
        (&req_data.dependencies, Type::JSONB),
      ]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  Ok(HttpResponse::Ok().json(json!({ "success": true })))
}

#[derive(Debug, MultipartForm)]
struct VerifUploadForm {
  #[multipart(limit = "1MB")]
  file: TempFile,
  filename: Text<String>,
}

#[post("/verify/{address}/upload")]
async fn upload_file(
  path: web::Path<String>,
  req: HttpRequest,
  MultipartForm(mut form): MultipartForm<VerifUploadForm>,
  ctx: web::Data<Context>
) -> Result<HttpResponse, RespErr> {
  verify_auth_token(&req)?;
  let address = path.into_inner();
  debug!("Uploaded file {} with size: {}", form.file.file_name.unwrap(), form.file.size);
  debug!("Contract address {}, new filename: {}", &address, &form.filename.0);
  if form.file.size > 1024 * 1024 {
    return Err(RespErr::BadRequest { msg: String::from("Uploaded file size exceeds 1MB limit") });
  }
  let mut contents = String::new();
  match form.file.file.read_to_string(&mut contents) {
    Ok(_) => (),
    Err(e) => {
      error!("Failed to read uploaded file: {}", e.to_string());
      return Err(RespErr::BadRequest {
        msg: String::from("Failed to process uploaded file, most likely file is not in UTF-8 format."),
      });
    }
  }
  let can_upload: String = ctx.db
    .query(
      "SELECT vsc_cv.can_upload_file($1,$2);",
      &[
        (&address, Type::VARCHAR),
        (&form.filename.0, Type::VARCHAR),
      ]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?
    [0].get(0);
  if can_upload.len() > 0 {
    return Err(RespErr::BadRequest { msg: can_upload });
  }
  ctx.db
    .query(
      "INSERT INTO vsc_cv.source_code(contract_addr,fname,content) VALUES($1,$2,$3) ON CONFLICT(contract_addr,fname) DO UPDATE SET content=$3;",
      &[
        (&address, Type::VARCHAR),
        (&form.filename.0, Type::VARCHAR),
        (&contents, Type::VARCHAR),
      ]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  Ok(HttpResponse::Ok().json(json!({ "success": true })))
}

#[post("/verify/{address}/complete")]
async fn upload_complete(path: web::Path<String>, req: HttpRequest, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  verify_auth_token(&req)?;
  let address = path.into_inner();
  let contr = ctx.db
    .query("SELECT hive_username, status FROM vsc_cv.contracts WHERE contract_addr=$1;", &[(&address, Type::VARCHAR)]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if contr.len() < 1 {
    return Err(RespErr::BadRequest { msg: String::from("Contract does not exist") });
  }
  let status: i16 = contr[0].get(1);
  if status != 0 {
    return Err(RespErr::BadRequest { msg: String::from("Status is currently not pending upload") });
  }
  let file_count: i64 = ctx.db
    .query("SELECT COUNT(*) FROM vsc_cv.source_code WHERE contract_addr=$1;", &[(&address, Type::VARCHAR)]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?
    [0].get(0);
  if file_count < 1 {
    return Err(RespErr::BadRequest { msg: String::from("No source files were uploaded for this contract") });
  }
  ctx.db
    .query("UPDATE vsc_cv.contracts SET status=1::SMALLINT WHERE contract_addr=$1;", &[(&address, Type::VARCHAR)]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  ctx.compiler.notify();
  debug!("Complete");
  Ok(HttpResponse::Ok().json(json!({ "success": true })))
}

#[get("/languages")]
async fn list_langs(ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let rows = ctx.db
    .query("SELECT jsonb_agg(name) FROM vsc_cv.languages;", &[]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  let result: Value = rows[0].get(0);
  Ok(HttpResponse::Ok().json(result))
}

#[get("/licenses")]
async fn list_licenses(ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let rows = ctx.db
    .query("SELECT jsonb_agg(name) FROM vsc_cv.licenses;", &[]).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  let result: Value = rows[0].get(0);
  Ok(HttpResponse::Ok().json(result))
}

#[get("/contract/{address}")]
async fn contract_info(path: web::Path<String>, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let addr = path.into_inner();
  let contract = ctx.db
    .query(
      "SELECT c.bytecode_cid, c.hive_username, c.request_ts, c.verified_ts, s.name, c.exports, lc.name, lg.name, c.dependencies FROM vsc_cv.contracts c JOIN vsc_cv.status s ON s.id = c.status JOIN vsc_cv.licenses lc ON lc.id = c.license JOIN vsc_cv.languages lg ON lg.id = c.lang WHERE contract_addr=$1;",
      &[(&addr, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if contract.len() == 0 {
    return Ok(HttpResponse::NotFound().json(json!({"error": "contract not found"})));
  }
  let files = ctx.db
    .query(
      "SELECT COALESCE(jsonb_agg(fname), '[]'::jsonb) FROM vsc_cv.source_code WHERE contract_addr=$1 AND is_lockfile=false;",
      &[(&addr, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  let lockfilename = ctx.db
    .query(
      "SELECT fname FROM vsc_cv.source_code WHERE contract_addr=$1 AND is_lockfile=true LIMIT 1;", // assume only one lockfile per contract
      &[(&addr, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  let result =
    json!({
    "address": &addr,
    "code": contract[0].get::<usize, &str>(0),
    "username": contract[0].get::<usize, &str>(1),
    "request_ts": &contract[0].get::<usize, NaiveDateTime>(2).format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
    "verified_ts": &contract[0].get::<usize, Option<NaiveDateTime>>(3).map(|t| t.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
    "status": contract[0].get::<usize, &str>(4),
    "exports": contract[0].get::<usize, Option<Value>>(5),
    "files": files[0].get::<usize, Value>(0),
    "lockfile": match lockfilename.len() {
      0 => None,
      _ => Some(lockfilename[0].get::<usize, &str>(0)),
    },
    "license": contract[0].get::<usize, &str>(6),
    "lang": contract[0].get::<usize, &str>(7),
    "dependencies": contract[0].get::<usize, Value>(8)
  });
  Ok(HttpResponse::Ok().json(result))
}

#[get("/contract/{address}/files/ls")]
async fn contract_files_ls(path: web::Path<String>, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let addr = path.into_inner();
  let files = ctx.db
    .query(
      "SELECT jsonb_agg(fname) FROM vsc_cv.source_code WHERE contract_addr=$1 AND is_lockfile=false;",
      &[(&addr, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  Ok(HttpResponse::Ok().json(files[0].get::<usize, Value>(0)))
}

#[get("/contract/{address}/files/cat/{filename}")]
async fn contract_files_cat(path: web::Path<(String, String)>, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let (addr, filename) = path.into_inner();
  let files = ctx.db
    .query(
      "SELECT content FROM vsc_cv.source_code WHERE contract_addr=$1 AND fname=$2;",
      &[
        (&addr, Type::VARCHAR),
        (&filename, Type::VARCHAR),
      ]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if files.len() == 0 {
    return Ok(HttpResponse::NotFound().body("Error 404 file not found"));
  }
  Ok(HttpResponse::Ok().body(files[0].get::<usize, String>(0)))
}

#[get("/contract/{address}/files/catall")]
async fn contract_files_cat_all(path: web::Path<String>, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let addr = path.into_inner();
  let files = ctx.db
    .query(
      "SELECT jsonb_agg(jsonb_build_object('name',fname,'content',content)) FROM vsc_cv.source_code WHERE contract_addr=$1 AND is_lockfile=false;",
      &[(&addr, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if files.len() == 0 {
    return Ok(HttpResponse::NotFound().body("Error 404 file not found"));
  }
  Ok(HttpResponse::Ok().json(files[0].get::<usize, Value>(0)))
}

#[get("/bytecode/{cid}/lookupaddr")]
async fn bytecode_lookup_addr(path: web::Path<String>, ctx: web::Data<Context>) -> Result<HttpResponse, RespErr> {
  let cid = path.into_inner();
  let addr = ctx.db
    .query(
      "SELECT contract_addr FROM vsc_cv.contracts WHERE bytecode_cid=$1 AND status=3::SMALLINT LIMIT 1;",
      &[(&cid, Type::VARCHAR)]
    ).await
    .map_err(|e| RespErr::DbErr { msg: e.to_string() })?;
  if addr.len() == 0 {
    return Ok(HttpResponse::NotFound().json(json!({"error": "no matching contracts found"})));
  }
  Ok(HttpResponse::Ok().json(json!({"address": addr[0].get::<usize, &str>(0)})))
}
