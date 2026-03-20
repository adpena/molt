/**
 * Molt VFS Browser Adapter
 *
 * Provides in-memory virtual filesystem for Molt WASM modules running
 * in the browser. Implements /bundle (read-only), /tmp (read-write),
 * and /dev (stdio) mounts.
 */

class BundleFs {
    constructor(files) {
        // files: Map<string, Uint8Array>
        this.files = files instanceof Map ? files : new Map(Object.entries(files));
    }

    static async fromFetch(url) {
        // Fetch and untar a bundle.tar
        const response = await fetch(url);
        const buffer = await response.arrayBuffer();
        return BundleFs.fromTar(new Uint8Array(buffer));
    }

    static fromTar(tarBytes) {
        // Minimal tar parser — header is 512 bytes, content follows
        const files = new Map();
        let offset = 0;
        const decoder = new TextDecoder();

        while (offset < tarBytes.length - 512) {
            // Read header
            const header = tarBytes.subarray(offset, offset + 512);
            // Check for end-of-archive (two null blocks)
            if (header[0] === 0) break;

            // Extract filename (bytes 0-99, null-terminated)
            let nameEnd = 0;
            while (nameEnd < 100 && header[nameEnd] !== 0) nameEnd++;
            const name = decoder.decode(header.subarray(0, nameEnd));

            // Extract size (bytes 124-135, octal)
            const sizeStr = decoder.decode(header.subarray(124, 136)).trim();
            const size = parseInt(sizeStr, 8) || 0;

            // Extract type flag (byte 156)
            const typeFlag = header[156];

            offset += 512; // skip header

            if (typeFlag === 48 || typeFlag === 0) { // regular file ('0' or null)
                if (name && size > 0) {
                    files.set(name, new Uint8Array(tarBytes.buffer, tarBytes.byteOffset + offset, size));
                }
            }

            // Advance past content (rounded up to 512)
            offset += Math.ceil(size / 512) * 512;
        }

        return new BundleFs(files);
    }

    read(path) {
        const data = this.files.get(path);
        if (!data) throw new Error(`ENOENT: ${path}`);
        return data;
    }

    exists(path) { return this.files.has(path); }

    stat(path) {
        if (this.files.has(path)) {
            return { isFile: true, isDir: false, size: this.files.get(path).byteLength, readonly: true };
        }
        // Check if it's a directory prefix
        const prefix = path ? path + '/' : '';
        for (const key of this.files.keys()) {
            if (key.startsWith(prefix)) {
                return { isFile: false, isDir: true, size: 0, readonly: true };
            }
        }
        return null;
    }

    readdir(path) {
        const prefix = path ? path + '/' : '';
        const entries = new Set();
        for (const key of this.files.keys()) {
            if (key.startsWith(prefix)) {
                const rest = key.slice(prefix.length);
                const name = rest.split('/')[0];
                if (name) entries.add(name);
            }
        }
        return [...entries].sort();
    }
}

class TmpFs {
    constructor(quotaBytes = 64 * 1024 * 1024) {
        this.files = new Map();
        this.quota = quotaBytes;
        this.used = 0;
    }

    read(path) {
        const data = this.files.get(path);
        if (!data) throw new Error(`ENOENT: ${path}`);
        return data;
    }

    write(path, data) {
        const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
        const old = this.files.get(path);
        const delta = bytes.byteLength - (old ? old.byteLength : 0);
        if (this.used + delta > this.quota) {
            throw new Error('ENOSPC: /tmp quota exceeded');
        }
        this.files.set(path, new Uint8Array(bytes));
        this.used += delta;
    }

    exists(path) { return this.files.has(path); }

    unlink(path) {
        const data = this.files.get(path);
        if (data) {
            this.used -= data.byteLength;
            this.files.delete(path);
        }
    }

    clear() {
        this.files.clear();
        this.used = 0;
    }

    stat(path) {
        if (this.files.has(path)) {
            return { isFile: true, isDir: false, size: this.files.get(path).byteLength, readonly: false };
        }
        return null;
    }
}

class DevFs {
    constructor() {
        this.stdoutChunks = [];
        this.stderrChunks = [];
        this.stdinData = new Uint8Array(0);
        this.stdinOffset = 0;
    }

    setStdin(data) {
        this.stdinData = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
        this.stdinOffset = 0;
    }

    readStdin(n) {
        const end = Math.min(this.stdinOffset + n, this.stdinData.byteLength);
        const chunk = this.stdinData.subarray(this.stdinOffset, end);
        this.stdinOffset = end;
        return chunk;
    }

    writeStdout(data) { this.stdoutChunks.push(new Uint8Array(data)); }
    writeStderr(data) { this.stderrChunks.push(new Uint8Array(data)); }

    getStdout() { return concat(this.stdoutChunks); }
    getStderr() { return concat(this.stderrChunks); }

    clear() {
        this.stdoutChunks = [];
        this.stderrChunks = [];
        this.stdinData = new Uint8Array(0);
        this.stdinOffset = 0;
    }
}

function concat(chunks) {
    const total = chunks.reduce((s, c) => s + c.byteLength, 0);
    const result = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        result.set(chunk, offset);
        offset += chunk.byteLength;
    }
    return result;
}

/**
 * Main VFS class that aggregates all mounts.
 */
class MoltVfs {
    constructor() {
        this.bundle = null;
        this.tmp = new TmpFs();
        this.dev = new DevFs();
    }

    async loadBundle(url) {
        this.bundle = await BundleFs.fromFetch(url);
    }

    loadBundleFromTar(tarBytes) {
        this.bundle = BundleFs.fromTar(tarBytes);
    }

    loadBundleFromFiles(files) {
        this.bundle = new BundleFs(files);
    }

    resolve(path) {
        if (path.startsWith('/bundle/') || path === '/bundle') {
            const rel = path.length > 8 ? path.slice(8) : '';
            return { mount: this.bundle, rel, prefix: '/bundle' };
        }
        if (path.startsWith('/tmp/') || path === '/tmp') {
            const rel = path.length > 5 ? path.slice(5) : '';
            return { mount: this.tmp, rel, prefix: '/tmp' };
        }
        if (path.startsWith('/dev/')) {
            const rel = path.slice(5);
            return { mount: this.dev, rel, prefix: '/dev' };
        }
        return null;
    }

    clear() {
        this.tmp.clear();
        this.dev.clear();
    }
}

// Export for both module and global contexts
if (typeof module !== 'undefined') {
    module.exports = { MoltVfs, BundleFs, TmpFs, DevFs };
}
if (typeof globalThis !== 'undefined') {
    globalThis.MoltVfs = MoltVfs;
    globalThis.BundleFs = BundleFs;
    globalThis.TmpFs = TmpFs;
    globalThis.DevFs = DevFs;
}
