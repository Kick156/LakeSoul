# Native IO Performance Comparison Results

## 1. Parquet Scan

### Settings

Code: lakesoul-spark/src/test/scala/org/apache/spark/sql/lakesoul/benchmark/io/ParquetScanBenchmark.scala

Tested on Spark 3.3.1 with Parquet-mr 1.12.2, Arrow-rs(parquet) 31.0.0.
Parquet file size: 894.3MB, compressed with snappy. Metadata:

```
############ file meta data ############
created_by: parquet-mr version 1.12.2 (build 77e30c8093386ec52c3cfa6c34b7ef3321322c94)
num_columns: 8
num_rows: 10000000
num_row_groups: 7
format_version: 1.0
serialized_size: 7688
```

File is read with only one parallelism in Spark.

### Results
1. MinIO

|          | Parquet-mr | Native-IO | Improvement |
|----------|------------|-----------|-------------|
| Time(ms) | 11417      | 4381      | 2.61x       |

2. AWS S3

|          | Parquet-mr | Native-IO | Improvement |
|----------|------------|-----------|-------------|
| Time(ms) | 25190      | 6965      | 3.62x       |
