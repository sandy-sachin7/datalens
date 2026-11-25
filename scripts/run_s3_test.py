#!/usr/bin/env python3
"""Run `tests/cloud_s3.rs` against a local ephemeral S3-compatible HTTP server (Moto)."""
from __future__ import annotations

import os
import subprocess
import sys

import boto3
from moto.server import ThreadedMotoServer


def main() -> int:
    port = int(os.environ.get("DELTALENS_TEST_S3_PORT", "19000"))
    bucket = os.environ.get("DELTALENS_TEST_S3_BUCKET", "deltalens-test")
    endpoint = f"http://127.0.0.1:{port}"

    server = ThreadedMotoServer(ip_address="127.0.0.1", port=port, verbose=False)
    server.start()
    try:
        s3 = boto3.client(
            "s3",
            endpoint_url=endpoint,
            aws_access_key_id="testing",
            aws_secret_access_key="testing",
            region_name="us-east-1",
        )
        s3.create_bucket(Bucket=bucket)

        env = os.environ.copy()
        env.update(
            {
                "AWS_ACCESS_KEY_ID": "testing",
                "AWS_SECRET_ACCESS_KEY": "testing",
                "AWS_REGION": "us-east-1",
                "AWS_ENDPOINT": endpoint,
                "AWS_ALLOW_HTTP": "true",
                "DELTALENS_TEST_S3_BUCKET": bucket,
            }
        )
        proc = subprocess.run(
            ["cargo", "test", "--test", "cloud_s3", "--locked", "--", "--ignored", "--nocapture"],
            env=env,
        )
        return proc.returncode
    finally:
        server.stop()


if __name__ == "__main__":
    sys.exit(main())
