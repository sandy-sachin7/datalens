use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use deltalens::core::reader::DeltaLogReader;
use deltalens::core::storage::LocalStorage;

fn bench_log_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("log_parsing");

    for fixture in &["small", "medium", "large"] {
        let path = format!("benches/fixtures/{}", fixture);

        if !std::path::Path::new(&path).exists() {
            continue;
        }

        let storage = Box::new(LocalStorage::new(path.clone()));
        let reader = DeltaLogReader::new(storage, &path).unwrap();

        group.bench_with_input(
            BenchmarkId::new("read_all_commits", fixture),
            fixture,
            |b, _| {
                b.iter(|| reader.read_range(None, None).unwrap());
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_log_parsing);
criterion_main!(benches);
