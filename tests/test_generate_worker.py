def test_generate_worker_produces_valid_js(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=32)
    content = output.read_text()
    assert "fetch" in content
    assert "WebAssembly" in content
