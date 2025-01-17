use deadpool_postgres::{ Config, CreatePoolError, Manager, ManagerConfig, RecyclingMethod, Runtime };
use deadpool::managed::{ Pool, Object };
use tokio_postgres::{ types::ToSql, NoTls, Row, types::Type };
use sql_minifier::macros::load_sql;
use std::{ fmt, error };
use log::info;
use crate::config;

const PSQL_CREATE_TABLES: &str = load_sql!("src/sql/create_tables.sql");

#[derive(Debug)]
pub struct DbQueryError {
  message: String,
}

impl error::Error for DbQueryError {}
impl fmt::Display for DbQueryError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.message)
  }
}

#[derive(Clone)]
pub struct DbPool {
  pool: Pool<Manager>,
}

impl DbPool {
  pub fn init() -> Result<Self, CreatePoolError> {
    let mut cfg = Config::new();
    cfg.url = Some(config::config.psql_url.clone());
    cfg.manager = Some(ManagerConfig {
      recycling_method: RecyclingMethod::Fast,
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    Ok(Self { pool })
  }

  async fn get_client(&self) -> Result<Object<Manager>, DbQueryError> {
    match self.pool.get().await {
      Ok(v) => Ok(v),
      Err(e) => {
        return Err(DbQueryError { message: e.to_string() });
      }
    }
  }

  pub async fn setup(&self) -> Result<(), DbQueryError> {
    let loaded = self.query("SELECT 1 FROM pg_catalog.pg_tables WHERE schemaname='vsc_cv';", &[]).await?.len() > 0;
    if !loaded {
      info!("Setting up VSC contract verifier database...");
      self.execute_file(PSQL_CREATE_TABLES).await?;
    } else {
      info!("Connected to database successfully");
    }
    Ok(())
  }

  pub async fn query(&self, statement: &str, params: &[(&(dyn ToSql + Sync), Type)]) -> Result<Vec<Row>, DbQueryError> {
    let client = self.get_client().await?;
    match client.query_typed(statement, params).await {
      Ok(rows) => Ok(rows),
      Err(e) => Err(DbQueryError { message: e.to_string() }),
    }
  }

  pub async fn execute_file(&self, statement: &str) -> Result<(), DbQueryError> {
    let client = self.get_client().await?;
    match client.batch_execute(statement).await {
      Ok(_) => Ok(()),
      Err(e) => Err(DbQueryError { message: e.to_string() }),
    }
  }
}
