use atomic_refcell::AtomicRefCell;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::mem::ManuallyDrop;
use std::num::NonZeroUsize;
use std::sync::Arc;

use std::time::Instant; 
use std::future::Future;


use std::future::Ready;

use derivative::Derivative;


pub use datafusion::arrow::error::ArrowError;
pub use datafusion::arrow::error::Result as ArrowResult;
pub use datafusion::arrow::record_batch::RecordBatch;
pub use datafusion::error::{DataFusionError, Result};
use datafusion::physical_plan::RecordBatchStream;
use datafusion::execution::runtime_env::{RuntimeConfig, RuntimeEnv};
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::prelude::{SessionConfig, SessionContext};
use object_store::aws::{AmazonS3Builder};
use object_store::RetryConfig;

use futures::StreamExt;
use futures::stream::{Map, Buffered, Stream};
use core::pin::Pin;

use tokio::runtime::{Builder, Runtime};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
// use tokio_stream::StreamExt;

use crate::merge_logic::merge_partitioned_file::MergePartitionedFile;
use crate::filter::Parser as FilterParser;

#[derive(Derivative, Default)]
pub struct LakeSoulReaderConfig {
    // files to read
    files: Vec<String>,
    merge_files: Vec<MergePartitionedFile>,
    // primary key column names
    primary_keys: Vec<String>,
    // selecting columns
    columns: Vec<String>,
    schema: HashMap<String, String>,
    
    // filtering predicates
    filters: Vec<Expr>,
    batch_size: usize,

    // object store related configs
    object_store_options: HashMap<String, String>,

    // tokio runtime related configs
    #[derivative(Default(value = "2"))]
    thread_num: usize,

    #[derivative(Default(value = "1"))]
    buffer_size: usize,
}

pub struct LakeSoulReaderConfigBuilder {
    config: LakeSoulReaderConfig,
}

impl LakeSoulReaderConfigBuilder {
    pub fn new() -> Self {
        LakeSoulReaderConfigBuilder {
            config: LakeSoulReaderConfig::default(),
        }
    }

    pub fn with_file(mut self, file: String) -> Self {
        self.config.files.push(file);
        self
    }

    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.config.files = files;
        self
    }

    pub fn with_merge_file(mut self, file: MergePartitionedFile) -> Self {
        self.config.merge_files.push(file);
        self
    }

    pub fn with_merge_files(mut self, files: Vec<MergePartitionedFile>) -> Self {
        self.config.merge_files = files;
        self
    }

    pub fn with_primary_keys(mut self, pks: Vec<String>) -> Self {
        self.config.primary_keys = pks;
        self
    }

    pub fn with_column(mut self, col: String, datatype: String) -> Self {
        self.config.columns.push(String::from(&col));
        self.config.schema.insert(String::from(&col), datatype);
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.config.batch_size=batch_size;
        self
    }

    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.config.buffer_size=buffer_size;
        self
    }


    pub fn with_columns(mut self, cols: Vec<String>) -> Self {
        self.config.columns = cols;
        self
    }

    pub fn with_filter_str(mut self, filter_str: String) -> Self {
        // self.config.filters = filters;
        let expr = FilterParser::parse(filter_str, &self.config.schema);
        self.config.filters.push(expr);
        self
    }

    pub fn with_filters(mut self, filters: Vec<Expr>) -> Self {
        self.config.filters = filters;
        self
    }

    pub fn with_object_store_option(mut self, key: String, value: String) -> Self {
        self.config.object_store_options.insert(key, value);
        self
    }

    pub fn with_thread_num(mut self, thread_num: usize) -> Self {
        self.config.thread_num = thread_num;
        self
    }

    pub fn build(self) -> LakeSoulReaderConfig {
        self.config
    }
}

pub struct LakeSoulReader {
    sess_ctx: SessionContext,
    config: LakeSoulReaderConfig,
    // stream: Box<MaybeUninit<Pin<Box<dyn RecordBatchStream+Send>>>>,
    stream: Box<MaybeUninit<Pin<Box<dyn Stream<Item=ArrowResult<RecordBatch>>+Send>>>>,
}

impl LakeSoulReader {
    pub fn new(config: LakeSoulReaderConfig) -> Result<Self> {
        let sess_ctx = LakeSoulReader::create_session_context(&config)?;
        Ok(LakeSoulReader {
            sess_ctx,
            config,
            stream: Box::new_uninit(),
        })
    }

    fn check_fs_type_enabled(config: &LakeSoulReaderConfig, fs_name: &str) -> bool {
        if let Some(fs_enabled) = config
            .object_store_options
            .get(format!("fs.{}.enabled", fs_name).as_str())
        {
            return match fs_enabled.parse::<bool>() {
                Ok(enabled) => enabled,
                _ => false,
            };
        }
        false
    }

    fn register_s3_object_store(config: &LakeSoulReaderConfig, runtime: &RuntimeEnv) -> Result<()> {
        if !LakeSoulReader::check_fs_type_enabled(config, "s3") {
            return Ok(());
        }
        let key = config.object_store_options.get("fs.s3.access.key");
        let secret = config.object_store_options.get("fs.s3.access.secret");
        let region = config.object_store_options.get("fs.s3.region");
        let bucket = config.object_store_options.get("fs.s3.bucket");

        if region == None {
            return Err(DataFusionError::ArrowError(ArrowError::InvalidArgumentError(
                "missing fs.s3.region".to_string(),
            )));
        }

        if bucket == None {
            return Err(DataFusionError::ArrowError(ArrowError::InvalidArgumentError(
                "missing fs.s3.bucket".to_string(),
            )));
        }

        let endpoint = config.object_store_options.get("fs.s3.endpoint");
        let retry_config = RetryConfig {
            backoff: Default::default(),
            max_retries: 4,
            retry_timeout: Default::default()
        };
        let s3_store = AmazonS3Builder::new()
            .with_access_key_id(key.unwrap())
            .with_secret_access_key(secret.unwrap())
            .with_region(region.unwrap())
            .with_bucket_name(bucket.unwrap())
            .with_endpoint(endpoint.unwrap())
            .with_retry(retry_config)
            .with_allow_http(true)
            .build();
        runtime.register_object_store("s3", bucket.unwrap(), Arc::new(s3_store.unwrap()));
        // println!("{:?}", config.object_store_options);
        Ok(())
    }

    fn create_session_context(config: &LakeSoulReaderConfig) -> Result<SessionContext> {
        let sess_conf = SessionConfig::default()
            .with_batch_size(config.batch_size);
        let runtime = RuntimeEnv::new(RuntimeConfig::new())?;

        // register object store(s)
        LakeSoulReader::register_s3_object_store(config, &runtime)?;

        // create session context
        Ok(SessionContext::with_config_rt(sess_conf, Arc::new(runtime)))
    }

    pub async fn start(&mut self) -> Result<()> {
        let mut df = self
            .sess_ctx
            .read_parquet(self.config.files[0].as_str(), Default::default())
            .await?;
        if !self.config.columns.is_empty() {
            let cols: Vec<_> = self.config.columns.iter().map(String::as_str).collect();
            df = df.select_columns(&cols)?;
        }
        df = self.config.filters.iter().try_fold(df, |df, f| df.filter(f.clone()))?;
        // self.stream = Box::new(MaybeUninit::new(df.execute_stream().await?));
        // println!("{}", self.config.buffer_size);
        self.stream = Box::new(MaybeUninit::new(
            Box::pin(
                df.execute_stream().await?
            .map(std::future::ready).buffered(self.config.buffer_size))
        ));
        // let buffered_stream:Box<MaybeUninit<Buffered<_>>> = Box::new(MaybeUninit::new(df.execute_stream().await?));
        
        Ok(())
    }

    pub async fn next_rb(&mut self) -> Option<ArrowResult<RecordBatch>> {
        unsafe { self.stream.assume_init_mut().next().await }
    }


}

// Reader will be used in async closure sent to tokio
// while accessing its mutable methods.
pub struct SyncSendableMutableLakeSoulReader {
    inner: Arc<AtomicRefCell<Mutex<LakeSoulReader>>>,
    runtime: Arc<Runtime>,
}

impl SyncSendableMutableLakeSoulReader {
    pub fn new(reader: LakeSoulReader, runtime:Runtime) -> Self {
        SyncSendableMutableLakeSoulReader {
            inner: Arc::new(AtomicRefCell::new(Mutex::new(reader))),
            runtime: Arc::new(runtime),
        }
    }

    pub fn start_blocked(&self) -> Result<()> {
        let inner_reader = self.inner.clone();
        let runtime = self.get_runtime();
        runtime.block_on(async { 
            inner_reader.borrow().lock().await.start().await;
            Ok(())
         })
    }

    pub fn next_rb_callback(
        &self,
        f: Box<dyn FnOnce(Option<ArrowResult<RecordBatch>>) + Send + Sync>,
    ) -> JoinHandle<()> {
        let inner_reader = self.get_inner_reader();
        let runtime = self.get_runtime();
        runtime.spawn(async move {
            let reader = inner_reader.borrow();
            let mut reader = reader.lock().await;
            f(reader.next_rb().await);
        })
    }

    pub fn hello(&self) {
    }

    fn get_runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

    fn get_inner_reader(&self) -> Arc<AtomicRefCell<Mutex<LakeSoulReader>>>{
        self.inner.clone()
    }
}




#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::util::pretty::print_batches;
    use std::sync::mpsc::sync_channel;

    #[tokio::test]
    async fn test_reader_local() -> Result<()> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                // "/Users/ceng/base-0-0.parquet"
                // "/Users/ceng/part-00003-68b546de-5cc6-4abb-a8a9-f6af2e372791-c000.snappy.parquet"
                //"/Users/ceng/Documents/GitHub/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                "/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                .to_string()])
            .with_thread_num(1)
            .with_batch_size(256)
            .build();
        let mut reader = LakeSoulReader::new(reader_conf)?;
        let mut reader = ManuallyDrop::new(reader);
        reader.start().await?;
        static mut row_cnt: usize = 0;

        while let Some(rb) = reader.next_rb().await {
            // print_batches(std::slice::from_ref(&record_batch?))?;
            let num_rows = &rb.unwrap().num_rows();
            unsafe {
                row_cnt = row_cnt + num_rows;
                println!("{}", row_cnt);
            }
        }
        // unsafe{
        //     ManuallyDrop::drop(&mut reader);
        // }
        Ok(())
    }

    #[test]
    fn test_reader_local_blocked() -> Result<()> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                // "/Users/ceng/part-00003-68b546de-5cc6-4abb-a8a9-f6af2e372791-c000.snappy.parquet"
                "/Users/ceng/Documents/GitHub/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                    .to_string(),
            ])
            .with_thread_num(16)
            .with_batch_size(1)
            .build();
        let reader = LakeSoulReader::new(reader_conf)?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(reader.config.thread_num)
            .build()
            .unwrap();
        let reader = SyncSendableMutableLakeSoulReader::new(reader, runtime);
        reader.start_blocked()?;
        static mut row_cnt: usize = 0;
        loop {
            let (tx, rx) = sync_channel(1);
            let start = Instant::now();
            let f = move |rb: Option<ArrowResult<RecordBatch>>| match rb {
                None => tx.send(true).unwrap(),
                Some(rb) => {
                    // print_batches(std::slice::from_ref(&rb.unwrap())).unwrap();
                    let num_rows = &rb.unwrap().num_rows();
                    unsafe {
                        row_cnt = row_cnt + num_rows;
                        println!("{}", row_cnt);
                    }
                    println!("time cost: {:?} ms", start.elapsed().as_millis());// ms
                    tx.send(false).unwrap();
                }
            };
            reader.next_rb_callback(Box::new(f));
            let done = rx.recv().unwrap();
            if done {
                break;
            }
        }
        Ok(())
    }


    use datafusion::prelude::Partitioning;
    #[test]
    fn test_reader_partition() -> Result<()> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                "/Users/ceng/part-00003-68b546de-5cc6-4abb-a8a9-f6af2e372791-c000.snappy.parquet"
                // "/Users/ceng/Documents/GitHub/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                    .to_string(),
            ])
            .with_thread_num(1)
            .with_batch_size(8192)
            .build();
        let reader = LakeSoulReader::new(reader_conf)?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(reader.config.thread_num)
            .build()
            .unwrap();
        let reader = SyncSendableMutableLakeSoulReader::new(reader, runtime);
        reader.start_blocked()?;
        static mut row_cnt: usize = 0;
        loop {
            let (tx, rx) = sync_channel(1);
            let start = Instant::now();
            let f = move |rb: Option<ArrowResult<RecordBatch>>| match rb {
                None => tx.send(true).unwrap(),
                Some(rb) => {
                    let rb = rb.unwrap();
                    let num_rows = &rb.num_rows();
                    unsafe {
                        row_cnt = row_cnt + num_rows;
                        println!("{}", row_cnt);
                    }
                    // println!("time cost: {:?} ms", start.elapsed().as_millis());// ms
                    // print_batches(std::slice::from_ref(&rb)).unwrap();

                    tx.send(false).unwrap();
                }
            };
            reader.next_rb_callback(Box::new(f));
            let done = rx.recv().unwrap();
            if done {
                break;
            }
        }
        Ok(())
    }

    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn test_reader_s3() -> Result<()> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                // "s3://lakesoul-test-s3/base-0-0.parquet"
                "s3://lakesoul-test-s3/large_file.parquet"
                // "/Users/ceng/PycharmProjects/write_parquet/large_file.parquet"
                // "/Users/ceng/Documents/GitHub/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                // "s3://lakesoul-test-s3/part-00002-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                // "/Users/ceng/base-0-0.parquet"
                .to_string()])
            .with_thread_num(1)
            .with_batch_size(8192)
            .with_buffer_size(16)
            .with_object_store_option(String::from("fs.s3.enabled"), String::from("true"))
            .with_object_store_option(String::from("fs.s3.access.key"), String::from("minioadmin1"))
            .with_object_store_option(String::from("fs.s3.access.secret"), String::from("minioadmin1"))
            .with_object_store_option(String::from("fs.s3.region"), String::from("us-east-1"))
            .with_object_store_option(String::from("fs.s3.bucket"), String::from("lakesoul-test-s3"))
            .with_object_store_option(String::from("fs.s3.endpoint"), String::from("http://localhost:9002"))
            .build();
        let mut reader = LakeSoulReader::new(reader_conf)?;
        let mut reader = ManuallyDrop::new(reader);
        reader.start().await?;
        static mut row_cnt: usize = 0;

        let start = Instant::now();
        while let Some(rb) = reader.next_rb().await {
            // print_batches(std::slice::from_ref(&record_batch?))?;
            let num_rows = &rb.unwrap().num_rows();
            unsafe {
                row_cnt = row_cnt + num_rows;
                println!("{}", row_cnt);
            }
            sleep(Duration::from_millis(20)).await;
        }
        println!("time cost: {:?}ms", start.elapsed().as_millis());// ms
        
        Ok(())
    }

    #[test]
    fn test_reader_s3_blocked() -> Result<()> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                "s3://dmetasoul-bucket/yuchanghui/fsspec_benchmark/large_file_for_native_io_rgc20.parquet"
                // "s3://lakesoul-test-s3/large_file.parquet"
                // "/Users/ceng/PycharmProjects/write_parquet/large_file.parquet"
                // "/Users/ceng/Documents/GitHub/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                // "s3://lakesoul-test-s3/part-00002-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet"
                // "/Users/ceng/base-0-0.parquet"
                .to_string()])
            .with_thread_num(1)
            .with_batch_size(8192)
            .with_buffer_size(16)
            .with_object_store_option(String::from("fs.s3.enabled"), String::from("true"))
            .with_object_store_option(String::from("fs.s3.access.key"), String::from("WWCTQNZDHWMVZMJY9QJN"))
            .with_object_store_option(String::from("fs.s3.access.secret"), String::from("YoVuuQ9Qx7KYuODRyhWFqFxvEKKPQLjIaAm3aTam"))
            .with_object_store_option(String::from("fs.s3.region"), String::from("us-east-1"))
            .with_object_store_option(String::from("fs.s3.bucket"), String::from("dmetasoul-bucket"))
            .with_object_store_option(String::from("fs.s3.endpoint"), String::from("http://obs.cn-southwest-2.myhuaweicloud.com"))
            // .with_object_store_option(String::from("fs.s3.enabled"), String::from("true"))
            // .with_object_store_option(String::from("fs.s3.access.key"), String::from("minioadmin1"))
            // .with_object_store_option(String::from("fs.s3.access.secret"), String::from("minioadmin1"))
            // .with_object_store_option(String::from("fs.s3.region"), String::from("us-east-1"))
            // .with_object_store_option(String::from("fs.s3.bucket"), String::from("lakesoul-test-s3"))
            // .with_object_store_option(String::from("fs.s3.endpoint"), String::from("http://localhost:9002"))
            .build();
        let reader = LakeSoulReader::new(reader_conf)?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(reader.config.thread_num)
            .enable_all()
            .build()
            .unwrap();
        let reader = SyncSendableMutableLakeSoulReader::new(reader, runtime);
        reader.start_blocked()?;
        static mut row_cnt: usize = 0;
        loop {
            let (tx, rx) = sync_channel(1);
            let start = Instant::now();
            let f = move |rb: Option<ArrowResult<RecordBatch>>| match rb {
                None => tx.send(true).unwrap(),
                Some(rb) => {
                    // print_batches(std::slice::from_ref(&rb.unwrap())).unwrap();
                    let num_rows = &rb.unwrap().num_rows();
                    unsafe {
                        row_cnt = row_cnt + num_rows;
                        println!("{}", row_cnt);
                    }
                    println!("time cost: {:?} ms", start.elapsed().as_millis());// ms
                    tx.send(false).unwrap();
                }
            };
            reader.next_rb_callback(Box::new(f));
            let done = rx.recv().unwrap();
            if done {
                break;
            }
        }
        Ok(())
    }

    use parquet::arrow::{async_reader::ParquetRecordBatchStreamBuilder, ProjectionMask};
    // use arrow::util::pretty::print_batches;
    use futures::TryStreamExt;
    use tokio::fs::File;
    use datafusion::logical_expr::{col, lit, Expr, Operator};
    use datafusion_common::{ScalarValue, Column};

    async fn get_num_rows_of_file_with_filters(file_path: String, filters: Vec<Expr>) -> Result<usize> {
        let reader_conf = LakeSoulReaderConfigBuilder::new()
            .with_files(vec![
                file_path])
            .with_thread_num(1)
            .with_batch_size(32)
            .with_buffer_size(1)
            .with_filters(filters)
            .build();
        let mut reader = LakeSoulReader::new(reader_conf)?;
        let mut reader = ManuallyDrop::new(reader);
        reader.start().await?;
        let mut row_cnt: usize = 0;

        while let Some(rb) = reader.next_rb().await {
            // print_batches(std::slice::from_ref(&rb?))?;
            row_cnt += &rb.unwrap().num_rows();
        }

        Ok(row_cnt)
    }

    #[tokio::test]
    async fn test_expr_eq_neq() -> Result<()> {
        let mut filters1: Vec<Expr> = vec![];
        let v = ScalarValue::Utf8(Some("Amanda".to_string()));
        let filter = col("first_name").eq(Expr::Literal(v));
        filters1.push(filter);
        let mut row_cnt1 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters1).await;
        if let Ok(row_cnt) = result {
            row_cnt1 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters2: Vec<Expr> = vec![];
        let v = ScalarValue::Utf8(Some("Amanda".to_string()));
        let filter = col("first_name").not_eq(Expr::Literal(v));
        filters2.push(filter);

        let mut row_cnt2 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters2).await;
        if let Ok(row_cnt) = result {
            row_cnt2 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        assert_eq!(row_cnt1 + row_cnt2, 1000);

        Ok(())
    }

    #[tokio::test]
    async fn test_expr_lteq_gt() -> Result<()> {
        let mut filters1: Vec<Expr> = vec![];
        let v = ScalarValue::Float64(Some(139177.2));
        let filter = col("salary").lt_eq(Expr::Literal(v));
        filters1.push(filter);

        let mut row_cnt1 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters1).await;
        if let Ok(row_cnt) = result {
            row_cnt1 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters2: Vec<Expr> = vec![];
        let v = ScalarValue::Float64(Some(139177.2));
        let filter = col("salary").gt(Expr::Literal(v));
        filters2.push(filter);

        let mut row_cnt2 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters2).await;
        if let Ok(row_cnt) = result {
            row_cnt2 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters3: Vec<Expr> = vec![];
        let filter = col("salary").is_null();
        filters3.push(filter);

        let mut row_cnt3 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters3).await;
        if let Ok(row_cnt) = result {
            row_cnt3 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        assert_eq!(row_cnt1 + row_cnt2, 1000 - row_cnt3);

        Ok(())
    }

    #[tokio::test]
    async fn test_expr_null_notnull() -> Result<()> {
        let mut filters1: Vec<Expr> = vec![];
        let filter = col("cc").is_null();
        filters1.push(filter);

        let mut row_cnt1 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters1).await;
        if let Ok(row_cnt) = result {
            row_cnt1 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters2: Vec<Expr> = vec![];
        let filter = col("cc").is_not_null();
        filters2.push(filter);

        let mut row_cnt2 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters2).await;
        if let Ok(row_cnt) = result {
            row_cnt2 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        assert_eq!(row_cnt1 + row_cnt2, 1000);

        Ok(())
    }

    #[tokio::test]
    async fn test_expr_or_and() -> Result<()> {
        let mut filters1: Vec<Expr> = vec![];
        let first_name = ScalarValue::Utf8(Some("Amanda".to_string()));
        // let filter = col("first_name").eq(Expr::Literal(first_name)).and(col("last_name").eq(Expr::Literal(last_name)));
        let filter = col("first_name").eq(Expr::Literal(first_name));

        filters1.push(filter);
        let mut row_cnt1 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters1).await;
        if let Ok(row_cnt) = result {
            row_cnt1 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }
        // println!("{}", row_cnt1);

        let mut filters2: Vec<Expr> = vec![];
        let last_name = ScalarValue::Utf8(Some("Jordan".to_string()));
        // let filter = col("first_name").eq(Expr::Literal(first_name)).and(col("last_name").eq(Expr::Literal(last_name)));
        let filter = col("last_name").eq(Expr::Literal(last_name));

        filters2.push(filter);
        let mut row_cnt2 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters2).await;
        if let Ok(row_cnt) = result {
            row_cnt2 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters: Vec<Expr> = vec![];
        let first_name = ScalarValue::Utf8(Some("Amanda".to_string()));
        let last_name = ScalarValue::Utf8(Some("Jordan".to_string()));
        let filter = col("first_name").eq(Expr::Literal(first_name)).and(col("last_name").eq(Expr::Literal(last_name)));

        filters.push(filter);
        let mut row_cnt3 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters).await;
        if let Ok(row_cnt) = result {
            row_cnt3 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters: Vec<Expr> = vec![];
        let first_name = ScalarValue::Utf8(Some("Amanda".to_string()));
        let last_name = ScalarValue::Utf8(Some("Jordan".to_string()));
        let filter = col("first_name").eq(Expr::Literal(first_name)).or(col("last_name").eq(Expr::Literal(last_name)));

        filters.push(filter);
        let mut row_cnt4 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters).await;
        if let Ok(row_cnt) = result {
            row_cnt4 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        assert_eq!(row_cnt1 + row_cnt2 - row_cnt3, row_cnt4);

        Ok(())
    }

    #[tokio::test]
    async fn test_expr_not() -> Result<()> {
        let mut filters: Vec<Expr> = vec![];
        let filter = col("salary").is_null();
        filters.push(filter);
        let mut row_cnt1 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters).await;
        if let Ok(row_cnt) = result {
            row_cnt1 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        let mut filters: Vec<Expr> = vec![];
        let filter = Expr::not(col("salary").is_null());
        filters.push(filter);
        let mut row_cnt2 = 0;
        let result = get_num_rows_of_file_with_filters("file:/home/yuchanghui/syl_code/LakeSoul/native-io/lakesoul-io-java/src/test/resources/sample-parquet-files/part-00000-a9e77425-5fb4-456f-ba52-f821123bd193-c000.snappy.parquet".to_string(),
                                                       filters).await;
        if let Ok(row_cnt) = result {
            row_cnt2 = row_cnt;
        } else {
            assert_eq!(0, 1);
        }

        assert_eq!(row_cnt1 + row_cnt2, 1000);

        Ok(())
    }
}
