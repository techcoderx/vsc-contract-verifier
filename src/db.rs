use deadpool_postgres::{ Config, CreatePoolError, Manager, ManagerConfig, RecyclingMethod, Runtime };
use deadpool::managed::{ Pool, Object };
use tokio_postgres::{ types::{ ToSql, Type }, NoTls, Row };
use sql_minifier::minify_sql_file;
use std::{ fmt, error };
use log::info;

const PSQL_CREATE_TABLES: &str = minify_sql_file!("src/sql/create_tables.sql");
const PSQL_FX: &str = minify_sql_file!("src/sql/create_functions.sql");

#[derive(Debug)]
pub struct DbError {
  message: String,
}

impl error::Error for DbError {}
impl fmt::Display for DbError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.message)
  }
}

#[derive(Clone)]
pub struct DbPool {
  pool: Pool<Manager>,
}

impl DbPool {
  pub fn init(psql_url: String) -> Result<Self, CreatePoolError> {
    let mut cfg = Config::new();
    cfg.url = Some(psql_url);
    cfg.manager = Some(ManagerConfig {
      recycling_method: RecyclingMethod::Fast,
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    Ok(Self { pool })
  }

  async fn get_client(&self) -> Result<Object<Manager>, DbError> {
    let c = self.pool.get().await.map_err(|e| DbError { message: e.to_string() })?;
    Ok(c)
  }

  pub async fn setup(&self) -> Result<(), DbError> {
    let loaded = self.query("SELECT 1 FROM pg_catalog.pg_tables WHERE schemaname='vsc_cv';", &[]).await?.len() > 0;
    if !loaded {
      info!("Setting up VSC contract verifier database...");
      self.execute_file(PSQL_CREATE_TABLES).await?;
    } else {
      info!("Connected to database successfully");
    }
    self.execute_file(PSQL_FX).await?;
    Ok(())
  }

  pub async fn query(&self, statement: &str, params: &[(&(dyn ToSql + Sync), Type)]) -> Result<Vec<Row>, DbError> {
    let client = self.get_client().await?;
    let rows = client.query_typed(statement, params).await.map_err(|e| DbError { message: e.to_string() })?;
    Ok(rows)
  }

  pub async fn execute_file(&self, statement: &str) -> Result<(), DbError> {
    let client = self.get_client().await?;
    client.batch_execute(statement).await.map_err(|e| DbError { message: e.to_string() })?;
    Ok(())
  }
}
