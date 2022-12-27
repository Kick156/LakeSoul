docker exec -ti lakesoul-compose-huazeng-local-env-lakesoul-meta-db-1 psql -h localhost -U lakesoul_test -d lakesoul_test -f /meta_cleanup.sql
docker run --net lakesoul-compose-huazeng-local-env_default --rm -t swr.cn-north-4.myhuaweicloud.com/dmetasoul-repo/spark:v3.1.2 aws --no-sign-request --endpoint-url http://minio:9000 s3 rm --recursive s3://ccf-datalake-contest/
docker run --net lakesoul-compose-huazeng-local-env_default --rm -t swr.cn-north-4.myhuaweicloud.com/dmetasoul-repo/spark:v3.1.2 -v /home/huazeng/large_file.parquet:/opt/spark/work-dir/large.parquet aws --no-sign-request --endpoint-url http://minio:9000 s3 cp /opt/spark/work-dir/large.parquet s3://ccf-datalake-contest/
docker run --net lakesoul-compose-huazeng-local-env_default --rm -t -v /home/huazeng/Git/LakeSoul/temp/work-dir:/opt/spark/work-dir  -v /home/huazeng/Git/LakeSoul/temp/test:/home/huazeng/test -v /home/huazeng/large_file.parquet:/opt/spark/work-dir/large.parquet --env RUST_BACKTRACE=1 --env LakeSoulLib=/opt/spark/work-dir --env lakesoul_home=/opt/spark/work-dir/lakesoul.properties swr.cn-north-4.myhuaweicloud.com/dmetasoul-repo/spark:v3.1.2 spark-submit --driver-memory 3g  --driver-java-options "--add-opens=java.base/java.nio=ALL-UNNAMED  -XX:MaxPermSize=512M"  --class NativeTest aws-test-1.0-SNAPSHOT.jar --localtest
#docker run --net lakesoul-compose-huazeng-local-env_default --rm -t -v /home/huazeng/Git/LakeSoul/temp/work-dir:/opt/spark/work-dir  -v /home/huazeng/Git/LakeSoul/temp/test:/home/huazeng/test -v /home/huazeng/Git/LakeSoul/lakesoul-spark/target/lakesoul-spark-2.1.0-spark-3.1.2-SNAPSHOT.jar:/opt/spark/jars/lakesoul-spark-2.1.0-spark-3.1.2-SNAPSHOT.jar -v /home/huazeng/Git/LakeSoul/lakesoul-common/target/lakesoul-common-2.1.0-SNAPSHOT.jar:/opt/spark/jars/lakesoul-common-2.1.0-SNAPSHOT.jar  -v /home/huazeng/Git/LakeSoul/native-io/lakesoul-io-java/target/lakesoul-io-java-2.1.1-alpha-SNAPSHOT.jar:/opt/spark/jars/lakesoul-io-java-2.1.1-alpha-SNAPSHOT.jar -v /home/huazeng/.m2/repository/com/zaxxer/HikariCP/4.0.3/HikariCP-4.0.3.jar:/opt/spark/jars/HikariCP-4.0.3.jar -v /home/huazeng/.m2/repository/org/postgresql/postgresql/42.5.0/postgresql-42.5.0.jar:/opt/spark/jars/postgresql-42.5.0.jar -v /home/huazeng/.m2/repository/com/alibaba/fastjson/1.2.83/fastjson-1.2.83.jar:/opt/spark/jars/fastjson-1.2.83.jar -v /home/huazeng/.m2/repository/org/ow2/asm/asm/9.2/asm-9.2.jar:/opt/spark/jars/asm-9.2.jar -v /home/huazeng/large_file.parquet:/opt/spark/work-dir/large.parquet --env RUST_BACKTRACE=1 --env LakeSoulLib=/opt/spark/work-dir --env lakesoul_home=/opt/spark/work-dir/lakesoul.properties swr.cn-north-4.myhuaweicloud.com/dmetasoul-repo/spark:v3.1.2 spark-submit --driver-memory 3g  --driver-java-options "--add-opens=java.base/java.nio=ALL-UNNAMED  -XX:MaxPermSize=512M"  --class AwsTest aws-test-1.0-SNAPSHOT.jar --localtest
