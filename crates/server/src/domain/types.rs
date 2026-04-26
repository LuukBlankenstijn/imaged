use crate::error::Result;
use std::pin::Pin;

use tokio_stream::Stream;

pub type AppStream<T> = Pin<Box<dyn Stream<Item = Result<T>> + Send>>;

pub enum Task {}
