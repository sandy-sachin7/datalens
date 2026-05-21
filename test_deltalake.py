#!/usr/bin/env python3
"""Test script to create Delta Lake tables and exercise DeltaLens commands."""

import os
from deltalake.writer import write_deltalake
import pyarrow as pa

DATA_DIR = "/home/sachin/Desktop/Code/datalens/test_tables"

def create_sales_table():
    """Create initial sales transactions table."""
    table_path = f"{DATA_DIR}/sales"
    print(f"\n{'='*60}")
    print("CREATING: sales table (version 0)")
    print('='*60)

    transaction_ids = pa.array([1001, 1002, 1003, 1004, 1005], pa.int64())
    customer_ids = pa.array([501, 502, 503, 501, 504], pa.int64())
    product_ids = pa.array([2001, 2002, 2001, 2003, 2002], pa.int64())
    amounts = pa.array([29.99, 149.99, 29.99, 599.99, 149.99], pa.float64())
    regions = pa.array(["NORTH", "SOUTH", "NORTH", "EAST", "SOUTH"], pa.string())
    timestamps = pa.array([
        "2026-05-15T10:30:00", "2026-05-15T11:45:00", "2026-05-16T09:15:00",
        "2026-05-17T14:20:00", "2026-05-18T16:00:00"
    ], pa.string())

    table = pa.table({
        "transaction_id": transaction_ids,
        "customer_id": customer_ids,
        "product_id": product_ids,
        "amount": amounts,
        "region": regions,
        "timestamp": timestamps
    })

    write_deltalake(table_path, table, mode="overwrite")
    print(f"Created {table_path} with 5 records")
    return table_path


def add_returns_table():
    """Create returns table."""
    table_path = f"{DATA_DIR}/returns"
    print(f"\n{'='*60}")
    print("CREATING: returns table (version 0)")
    print('='*60)

    table = pa.table({
        "return_id": pa.array([9001, 9002], pa.int64()),
        "transaction_id": pa.array([1003, 1005], pa.int64()),
        "return_amount": pa.array([29.99, 149.99], pa.float64()),
        "return_reason": pa.array(["defective", "wrong_size"], pa.string()),
        "return_date": pa.array(["2026-05-20", "2026-05-21"], pa.string())
    })

    write_deltalake(table_path, table, mode="overwrite")
    print(f"Created {table_path} with 2 records")
    return table_path


def create_customers_table():
    """Create customers table."""
    table_path = f"{DATA_DIR}/customers"
    print(f"\n{'='*60}")
    print("CREATING: customers table (version 0)")
    print('='*60)

    table = pa.table({
        "customer_id": pa.array([501, 502, 503, 504, 505, 506], pa.int64()),
        "customer_name": pa.array(["Alice Johnson", "Bob Smith", "Carol White", "David Brown", "Eve Davis", "Frank Miller"], pa.string()),
        "email": pa.array(["alice@email.com", "bob@email.com", "carol@email.com", "david@email.com", "eve@email.com", "frank@email.com"], pa.string()),
        "signup_date": pa.array(["2025-01-15", "2025-03-22", "2025-06-10", "2025-08-05", "2025-11-20", "2026-01-08"], pa.string()),
        "country": pa.array(["USA", "USA", "Canada", "UK", "Germany", "France"], pa.string())
    })

    write_deltalake(table_path, table, mode="overwrite")
    print(f"Created {table_path} with 6 records")
    return table_path


def insert_new_sales(table_path):
    """Insert new records into sales table."""
    print(f"\n{'='*60}")
    print("OPERATION: Append new records (version 1)")
    print('='*60)

    table = pa.table({
        "transaction_id": pa.array([1006, 1007], pa.int64()),
        "customer_id": pa.array([505, 506], pa.int64()),
        "product_id": pa.array([2004, 2001], pa.int64()),
        "amount": pa.array([89.99, 34.99], pa.float64()),
        "region": pa.array(["WEST", "NORTH"], pa.string()),
        "timestamp": pa.array(["2026-05-19T10:00:00", "2026-05-19T12:30:00"], pa.string())
    })

    write_deltalake(table_path, table, mode="append")
    print(f"Appended 2 new records (transaction_id: 1006, 1007)")


def update_via_overwrite(table_path):
    """Simulate update by overwriting with modified data (version 2)."""
    print(f"\n{'='*60}")
    print("OPERATION: Overwrite with updated data (version 2)")
    print('='*60)

    table = pa.table({
        "transaction_id": pa.array([1001, 1002, 1003, 1004, 1005, 1006, 1007], pa.int64()),
        "customer_id": pa.array([501, 502, 503, 501, 504, 505, 506], pa.int64()),
        "product_id": pa.array([2001, 2002, 2001, 2003, 2002, 2004, 2001], pa.int64()),
        "amount": pa.array([39.99, 149.99, 29.99, 599.99, 149.99, 89.99, 34.99], pa.float64()),
        "region": pa.array(["NORTH", "SOUTH", "NORTH", "EAST", "SOUTH", "WEST", "NORTH"], pa.string()),
        "timestamp": pa.array([
            "2026-05-15T10:30:00", "2026-05-15T11:45:00", "2026-05-16T09:15:00",
            "2026-05-17T14:20:00", "2026-05-18T16:00:00", "2026-05-19T10:00:00", "2026-05-19T12:30:00"
        ], pa.string())
    })

    write_deltalake(table_path, table, mode="overwrite")
    print(f"Overwritten with 7 records (price updated for txn 1001: $29.99 -> $39.99)")


def delete_and_modify(table_path):
    """Simulate delete by overwriting without deleted records (version 3)."""
    print(f"\n{'='*60}")
    print("OPERATION: Delete record via overwrite (version 3)")
    print('='*60)

    table = pa.table({
        "transaction_id": pa.array([1001, 1002, 1003, 1004, 1005, 1006], pa.int64()),
        "customer_id": pa.array([501, 502, 503, 501, 504, 505], pa.int64()),
        "product_id": pa.array([2001, 2002, 2001, 2003, 2002, 2004], pa.int64()),
        "amount": pa.array([39.99, 149.99, 29.99, 599.99, 149.99, 89.99], pa.float64()),
        "region": pa.array(["NORTH", "SOUTH", "NORTH", "EAST", "SOUTH", "WEST"], pa.string()),
        "timestamp": pa.array([
            "2026-05-15T10:30:00", "2026-05-15T11:45:00", "2026-05-16T09:15:00",
            "2026-05-17T14:20:00", "2026-05-18T16:00:00", "2026-05-19T10:00:00"
        ], pa.string())
    })

    write_deltalake(table_path, table, mode="overwrite")
    print(f"Overwritten with 6 records (deleted transaction_id=1007)")


def schema_evolution(table_path):
    """Add new columns via schema evolution."""
    print(f"\n{'='*60}")
    print("OPERATION: Schema evolution - add columns (version 4)")
    print('='*60)

    table = pa.table({
        "transaction_id": pa.array([1001, 1002, 1003, 1004, 1005, 1006], pa.int64()),
        "customer_id": pa.array([501, 502, 503, 501, 504, 505], pa.int64()),
        "product_id": pa.array([2001, 2002, 2001, 2003, 2002, 2004], pa.int64()),
        "amount": pa.array([39.99, 149.99, 29.99, 599.99, 149.99, 89.99], pa.float64()),
        "region": pa.array(["NORTH", "SOUTH", "NORTH", "EAST", "SOUTH", "WEST"], pa.string()),
        "timestamp": pa.array([
            "2026-05-15T10:30:00", "2026-05-15T11:45:00", "2026-05-16T09:15:00",
            "2026-05-17T14:20:00", "2026-05-18T16:00:00", "2026-05-19T10:00:00"
        ], pa.string()),
        "discount_code": pa.array(["SAVE5", "FREESHIP", None, "SAVE10", None, None], pa.string()),
        "loyalty_tier": pa.array(["gold", "silver", "bronze", "gold", "silver", "bronze"], pa.string())
    })

    write_deltalake(table_path, table, mode="overwrite", schema_mode="merge")
    print(f"Schema evolved: added columns discount_code, loyalty_tier")


if __name__ == "__main__":
    os.makedirs(DATA_DIR, exist_ok=True)

    print("\n" + "="*60)
    print("DELTA LAKE TEST DATA GENERATION")
    print("="*60)

    sales_path = create_sales_table()
    add_returns_table()
    customers_path = create_customers_table()

    insert_new_sales(sales_path)
    update_via_overwrite(sales_path)
    delete_and_modify(sales_path)
    schema_evolution(sales_path)

    print("\n" + "="*60)
    print("TEST DATA CREATION COMPLETE")
    print("="*60)
    print(f"\nTables created in: {DATA_DIR}")
    print(f"  - sales (5 versions)")
    print(f"  - returns (1 version)")
    print(f"  - customers (1 version)")