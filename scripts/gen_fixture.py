"""
Generates synthetic Delta log fixtures for benchmarking and testing.
Creates deterministic, reproducible test data.
"""
import json
import os
import uuid
from datetime import datetime, timedelta

def generate_commit(version: int, num_files: int, operation: str = "WRITE") -> list[dict]:
    """Generate a single commit with num_files Add actions."""
    timestamp = int((datetime(2025, 1, 1) + timedelta(hours=version)).timestamp() * 1000)
    
    actions = []
    
    # CommitInfo
    actions.append({
        "commitInfo": {
            "timestamp": timestamp,
            "operation": operation,
            "operationParameters": {"mode": "Append"},
            "engineInfo": "Apache-Spark/3.5.0 Delta-Lake/3.1.0",
            "isBlindAppend": True,
            "operationMetrics": {
                "numFiles": str(num_files),
                "numOutputRows": str(num_files * 50000),
                "numOutputBytes": str(num_files * 17_000_000),
            }
        }
    })
    
    # Add actions
    for i in range(num_files):
        # Intentionally add skew for realism
        size = 17_000_000 + (i * 100_000 if i % 7 == 0 else 0)
        actions.append({
            "add": {
                "path": f"part-{version:05d}-{uuid.uuid4()}.parquet",
                "size": size,
                "modificationTime": timestamp,
                "dataChange": True,
                "partitionValues": {}
            }
        })
    
    return actions

def generate_table(output_dir: str, num_versions: int, files_per_version: int):
    """Generate a complete Delta table log fixture."""
    log_dir = os.path.join(output_dir, "_delta_log")
    os.makedirs(log_dir, exist_ok=True)
    
    for version in range(num_versions):
        actions = generate_commit(version, files_per_version)
        filename = f"{version:020d}.json"
        filepath = os.path.join(log_dir, filename)
        
        with open(filepath, "w") as f:
            for action in actions:
                f.write(json.dumps(action) + "\n")
    
    print(f"Generated {num_versions} commits with {files_per_version} files each")
    print(f"Total log files: {num_versions}")
    print(f"Total Add actions: {num_versions * files_per_version}")

if __name__ == "__main__":
    # Ensure fixtures directory exists
    os.makedirs("benches/fixtures", exist_ok=True)

    # Small fixture — fast tests
    print("Generating small fixture...")
    generate_table("benches/fixtures/small", num_versions=100, files_per_version=10)
    
    # Medium fixture — typical table
    print("Generating medium fixture...")
    generate_table("benches/fixtures/medium", num_versions=1000, files_per_version=50)

    # Large fixture — enterprise-scale stress-test
    print("Generating large fixture...")
    generate_table("benches/fixtures/large", num_versions=5000, files_per_version=100)
    
    print("Done generating fixtures.")
