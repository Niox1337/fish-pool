//! Thin helpers over the D1 binding.

use crate::error::{ApiError, ApiResult};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement};

/// Converts Rust values into D1 bind parameters.
pub trait Bind {
    fn js(&self) -> JsValue;
}

impl Bind for &str {
    fn js(&self) -> JsValue {
        JsValue::from_str(self)
    }
}
impl Bind for String {
    fn js(&self) -> JsValue {
        JsValue::from_str(self)
    }
}
impl Bind for &String {
    fn js(&self) -> JsValue {
        JsValue::from_str(self)
    }
}
impl Bind for i64 {
    fn js(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}
impl Bind for bool {
    fn js(&self) -> JsValue {
        JsValue::from_f64(if *self { 1.0 } else { 0.0 })
    }
}
impl<T: Bind> Bind for Option<T> {
    fn js(&self) -> JsValue {
        match self {
            Some(v) => v.js(),
            None => JsValue::NULL,
        }
    }
}

#[macro_export]
macro_rules! args {
    ($($v:expr),* $(,)?) => {
        &[$($crate::db::Bind::js(&$v)),*]
    };
}

#[derive(Deserialize)]
struct Count {
    n: i64,
}

pub struct Db(pub D1Database);

fn db_err(e: worker::Error) -> ApiError {
    let text = e.to_string();
    if text.contains("UNIQUE constraint failed") {
        return ApiError::conflict("conflict", "数据已存在");
    }
    ApiError::internal(format!("d1: {text}"))
}

impl Db {
    pub fn stmt(&self, sql: &str, args: &[JsValue]) -> ApiResult<D1PreparedStatement> {
        self.0.prepare(sql).bind(args).map_err(db_err)
    }

    pub async fn first<T: DeserializeOwned>(&self, sql: &str, args: &[JsValue]) -> ApiResult<Option<T>> {
        self.stmt(sql, args)?.first::<T>(None).await.map_err(db_err)
    }

    pub async fn all<T: DeserializeOwned>(&self, sql: &str, args: &[JsValue]) -> ApiResult<Vec<T>> {
        let res = self.stmt(sql, args)?.all().await.map_err(db_err)?;
        res.results::<T>().map_err(db_err)
    }

    /// Runs a statement and returns the number of changed rows.
    pub async fn run(&self, sql: &str, args: &[JsValue]) -> ApiResult<i64> {
        let res = self.stmt(sql, args)?.run().await.map_err(db_err)?;
        Ok(res.meta().ok().flatten().and_then(|m| m.changes).map(|c| c as i64).unwrap_or(0))
    }

    /// Runs `SELECT COUNT(*) AS n ...` style queries.
    pub async fn count(&self, sql: &str, args: &[JsValue]) -> ApiResult<i64> {
        Ok(self.first::<Count>(sql, args).await?.map(|c| c.n).unwrap_or(0))
    }

    /// Runs statements in one transaction and returns the changed row count of each.
    pub async fn batch(&self, stmts: Vec<D1PreparedStatement>) -> ApiResult<Vec<i64>> {
        if stmts.is_empty() {
            return Ok(Vec::new());
        }
        let results = self.0.batch(stmts).await.map_err(db_err)?;
        Ok(results
            .iter()
            .map(|r| r.meta().ok().flatten().and_then(|m| m.changes).map(|c| c as i64).unwrap_or(0))
            .collect())
    }
}
