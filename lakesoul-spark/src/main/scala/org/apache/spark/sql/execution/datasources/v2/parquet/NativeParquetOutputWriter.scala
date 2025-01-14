/*
 * Copyright [2022] [DMetaSoul Team]
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package org.apache.spark.sql.execution.datasources.v2.parquet

import org.apache.arrow.lakesoul.io.NativeIOWriter
import org.apache.arrow.lakesoul.memory.ArrowMemoryUtils
import org.apache.arrow.memory.BufferAllocator
import org.apache.arrow.vector.VectorSchemaRoot
import org.apache.arrow.vector.types.pojo.Schema
import org.apache.hadoop.conf.Configuration
import org.apache.hadoop.fs.{FileSystem, Path}
import org.apache.hadoop.fs.s3a.S3AFileSystem
import org.apache.hadoop.mapreduce.TaskAttemptContext
import org.apache.spark.sql.catalyst.InternalRow
import org.apache.spark.sql.execution.arrow.ArrowWriter
import org.apache.spark.sql.execution.datasources.OutputWriter
import org.apache.spark.sql.internal.SQLConf
import org.apache.spark.sql.lakesoul.sources.LakeSoulSQLConf
import org.apache.spark.sql.types.StructType
import org.apache.spark.sql.util.ArrowUtils

class NativeParquetOutputWriter(val path: String, dataSchema: StructType, timeZoneId: String, context: TaskAttemptContext) extends OutputWriter {

  val NATIVE_IO_WRITE_MAX_ROW_GROUP_SIZE = SQLConf.get.getConf(LakeSoulSQLConf.NATIVE_IO_WRITE_MAX_ROW_GROUP_SIZE)

  private var recordCount = 0

  val arrowSchema: Schema = ArrowUtils.toArrowSchema(dataSchema, timeZoneId)
  val nativeIOWriter: NativeIOWriter = new NativeIOWriter(arrowSchema)
  nativeIOWriter.addFile(path)

  val conf: Configuration = context.getConfiguration
  val fileSystem: FileSystem = new Path(path).getFileSystem(conf)
  if (fileSystem.isInstanceOf[S3AFileSystem]) {
    val s3aAccessKey = conf.get("fs.s3a.access.key")
    val s3aAccessSecret = conf.get("fs.s3a.access.secret")
    val s3aRegion = conf.get("fs.s3a.endpoint.region");
    val s3aEndpoint = conf.get("fs.s3a.endpoint")
    nativeIOWriter.setObjectStoreOption("fs.s3a.access.key", s3aAccessKey)
    nativeIOWriter.setObjectStoreOption("fs.s3a.access.secret", s3aAccessSecret)
    nativeIOWriter.setObjectStoreOption("fs.s3a.endpoint.region", s3aRegion)
    nativeIOWriter.setObjectStoreOption("fs.s3a.endpoint", s3aEndpoint)
  }

  nativeIOWriter.initializeWriter()

  val allocator: BufferAllocator =
    ArrowMemoryUtils.rootAllocator.newChildAllocator("toBatchIterator", 0, Long.MaxValue)
  val root: VectorSchemaRoot = VectorSchemaRoot.create(arrowSchema, allocator)

  val recordWriter: ArrowWriter = ArrowWriter.create(root)

  override def write(row: InternalRow): Unit = {

    recordWriter.write(row)
    recordCount += 1

    if (recordCount >= NATIVE_IO_WRITE_MAX_ROW_GROUP_SIZE) {
      recordWriter.finish()
      nativeIOWriter.write(root)
      recordWriter.reset()
      recordCount = 0
    }
  }


  override def close(): Unit = {

    recordWriter.finish()

    nativeIOWriter.write(root)
    nativeIOWriter.close()

    recordWriter.reset()
    root.close()
    allocator.close()

  }
}
