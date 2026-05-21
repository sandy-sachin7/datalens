#!/usr/bin/env python3
"""Stress test: generate a large Delta Lake table with 500+ versions."""

import os, sys, time, random
from deltalake.writer import write_deltalake
import pyarrow as pa

DATA_DIR = "/home/sachin/Desktop/Code/datalens/stress_tables"
os.makedirs(DATA_DIR, exist_ok=True)

def generate_batch(batch_id, size=50):
    """Generate a batch of sales records."""
    customers = random.choices([501, 502, 503, 504, 505, 506], k=size)
    products = random.choices([2001, 2002, 2003, 2004], k=size)
    amounts = [round(random.uniform(10.0, 600.0), 2) for _ in range(size)]
    regions = random.choices(["NORTH", "SOUTH", "EAST", "WEST"], k=size)
    txn_start = batch_id * 1000
    return {
        "transaction_id": pa.array(range(txn_start, txn_start + size), pa.int64()),
        "customer_id": pa.array(customers, pa.int64()),
        "product_id": pa.array(products, pa.int64()),
        "amount": pa.array(amounts, pa.float64()),
        "region": pa.array(regions, pa.string()),
    }


def stress_test():
    table_path = f"{DATA_DIR}/stress"
    print(f"\n{'='*60}")
    print(f"STRESS TEST: Generating 500+ versions at {table_path}")
    print(f"{'='*60}")

    total_start = time.time()

    # Version 0: Initial write (100 records)
    print(f"\n[000] Initial write (100 records)")
    batch = generate_batch(0, 100)
    t = pa.table({
        "transaction_id": batch["transaction_id"],
        "customer_id": batch["customer_id"],
        "product_id": batch["product_id"],
        "amount": batch["amount"],
        "region": batch["region"],
        "timestamp": pa.array([f"2026-01-01T10:00:00" for _ in range(100)], pa.string()),
    })
    write_deltalake(table_path, t, mode="overwrite")

    # Versions 1-200: Appends (batch of 10 records each)
    for i in range(1, 201):
        batch = generate_batch(i, 10)
        t = pa.table({
            "transaction_id": batch["transaction_id"],
            "customer_id": batch["customer_id"],
            "product_id": batch["product_id"],
            "amount": batch["amount"],
            "region": batch["region"],
            "timestamp": pa.array([f"2026-01-{(i%30)+1:02d}T10:00:00" for _ in range(10)], pa.string()),
        })
        write_deltalake(table_path, t, mode="append")
        if i % 50 == 0:
            print(f"  [{(i):03d}] Appended 10 records")

    # Versions 201-350: Overwrites (simulating updates with varying counts)
    for i in range(201, 351):
        batch = generate_batch(i, random.randint(5, 20))
        t = pa.table({
            "transaction_id": batch["transaction_id"],
            "customer_id": batch["customer_id"],
            "product_id": batch["product_id"],
            "amount": batch["amount"],
            "region": batch["region"],
            "timestamp": pa.array([f"2026-02-{(i%28)+1:02d}T10:00:00" for _ in range(len(batch["transaction_id"]))], pa.string()),
        })
        write_deltalake(table_path, t, mode="overwrite")
        if i % 50 == 0:
            print(f"  [{(i):03d}] Overwritten with {len(batch['transaction_id'])} records")

    # Versions 351-500: Schema evolution writes (adding/removing columns)
    for i in range(351, 501):
        batch = generate_batch(i, random.randint(5, 15))
        extra_cols = {
            "discount_code": pa.array(random.choices([None, "SAVE5", "FREESHIP", "SAVE10"], k=len(batch["transaction_id"])), pa.string()),
            "loyalty_tier": pa.array(random.choices(["bronze", "silver", "gold", "platinum"], k=len(batch["transaction_id"])), pa.string()),
        }
        t = pa.table({
            "transaction_id": batch["transaction_id"],
            "customer_id": batch["customer_id"],
            "product_id": batch["product_id"],
            "amount": batch["amount"],
            "region": batch["region"],
            "timestamp": pa.array([f"2026-03-{(i%31)+1:02d}T10:00:00" for _ in range(len(batch["transaction_id"]))], pa.string()),
            "discount_code": extra_cols["discount_code"],
            "loyalty_tier": extra_cols["loyalty_tier"],
        })
        write_deltalake(table_path, t, mode="overwrite", schema_mode="merge")
        if i % 50 == 0:
            print(f"  [{(i):03d}] Schema-evolved overwrite ({len(batch['transaction_id'])} records)")

    total_elapsed = time.time() - total_start
    version_count = len(os.listdir(os.path.join(table_path, "_delta_log")))
    print(f"\n{'='*60}")
    print(f"STRESS TEST COMPLETE")
    print(f"  Versions created: {version_count}")
    print(f"  Time elapsed: {total_elapsed:.2f}s")
    print(f"  Tables dir: {table_path}")
    print(f"{'='*60}")
    return table_path


if __name__ == "__main__":
    stress_test()