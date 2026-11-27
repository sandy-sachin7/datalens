#!/usr/bin/env python3
"""Run `tests/cloud_gcs.rs` against a local ephemeral GCS XML API HTTP server."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import urllib.parse

import boto3
from moto.moto_server.werkzeug_app import DomainDispatcherApplication, create_backend_app
from werkzeug.serving import make_server


class StripGcsBearerMiddleware:
    """Normalize GCS XML API requests from object_store::gcp (disable_oauth=true)
    for Moto's S3/GCS XML API backend: strip empty `Bearer ` OAuth headers and
    decode percent-encoded path characters in RAW_URI/PATH_INFO."""

    def __init__(self, app):
        self.app = app

    def __call__(self, environ, start_response):
        if not environ.get("HTTP_AUTHORIZATION", "").startswith("AWS4-"):
            environ["HTTP_AUTHORIZATION"] = (
                "AWS4-HMAC-SHA256 Credential=testing/20261128/us-east-1/s3/aws4_request, "
                "SignedHeaders=host, Signature=00"
            )
        return self.app(environ, start_response)


def main() -> int:
    port = int(os.environ.get("DELTALENS_TEST_GCS_PORT", "19443"))
    bucket = os.environ.get("DELTALENS_TEST_GCS_BUCKET", "deltalenstests")
    endpoint = f"http://127.0.0.1:{port}"

    app = StripGcsBearerMiddleware(DomainDispatcherApplication(create_backend_app))
    httpd = make_server("127.0.0.1", port, app, threaded=True)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    try:
        boto3.client(
            "s3",
            endpoint_url=endpoint,
            aws_access_key_id="testing",
            aws_secret_access_key="testing",
            region_name="us-east-1",
        ).create_bucket(Bucket=bucket)

        env = os.environ.copy()
        env.update(
            {
                "GOOGLE_SERVICE_ACCOUNT_KEY": json.dumps(
                    {
                        "gcs_base_url": endpoint,
                        "disable_oauth": True,
                        "client_email": "",
                        "private_key": "",
                        "private_key_id": "",
                    }
                ),
                "DELTALENS_TEST_GCS_BUCKET": bucket,
            }
        )
        proc = subprocess.run(
            ["cargo", "test", "--test", "cloud_gcs", "--locked", "--", "--ignored", "--nocapture"],
            env=env,
        )
        return proc.returncode
    finally:
        httpd.shutdown()


if __name__ == "__main__":
    sys.exit(main())
