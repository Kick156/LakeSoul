package org.apache.spark.sql.vectorized

import org.apache.arrow.lakesoul.io.NativeIOWrapper
import org.apache.arrow.vector.{ValueVector, VectorSchemaRoot}
import org.apache.spark.sql.AnalysisException
import org.apache.spark.sql.catalyst.InternalRow
import org.apache.spark.sql.types.{BinaryType, BooleanType, StructField}

import scala.collection.JavaConverters._

class NativeIOUtils {

}

object NativeIOUtils{

  def asArrayColumnVector(vectorSchemaRoot: VectorSchemaRoot): Array[ColumnVector] = {
    asScalaIteratorConverter(vectorSchemaRoot.getFieldVectors.iterator())
      .asScala
      .toSeq
      .map(vector => {
//        println(vector.getField)
        asColumnVector(vector)
      })
      .toArray
  }

  def asArrowColumnVector(vector: ValueVector): OffHeapArrowColumnVector ={
    new OffHeapArrowColumnVector(vector)
  }

  def asColumnVector(vector: ValueVector): ColumnVector ={
    asArrowColumnVector(vector).asInstanceOf[ColumnVector]
  }
}