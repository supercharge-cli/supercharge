#!/usr/bin/env node
// Runs once after npm install/update. Reads the supercharge binary from the
// matching per-platform optional dependency (@supercharge-cli/supercharge-<platform>)
// and installs it to ~/.supercharge/bin/ using versioned filenames:
//
//   Unix:    supercharge-<version>  +  supercharge  (symlink)
//   Windows: supercharge-<version>.exe  +  supercharge.exe  (copy)
//
// Versioned files ensure running processes are never disrupted on macOS
// (replacing a binary that a running process has mmap'd causes SIGKILL
// because the kernel can no longer verify the code signature).
const path = require('path');
const fs = require('fs');
const os = require('os');
const zlib = require('zlib');
const { execSync } = require('child_process');
const TOML = require('@iarna/toml');

// $SUPERCHARGE_HOME (else ~/.supercharge), matching the Rust supercharge_home(): a symlinked
// $HOME resolves the same way. Lets fleets relocate the binary off a slow $HOME
// (NFS); old code hardcoded os.homedir().
function defaultSuperchargeHome() {
    const home = os.homedir();
    try { return path.join(fs.realpathSync(home), '.supercharge'); } catch { return path.join(home, '.supercharge'); }
}
const SUPERCHARGE_HOME = process.env.SUPERCHARGE_HOME ?? defaultSuperchargeHome();
const CANONICAL_DIR = path.join(SUPERCHARGE_HOME, 'bin');

const key = `${process.platform}-${process.arch}`;
const SUPPORTED = new Set([
    'darwin-arm64',
    'darwin-x64',
    'linux-x64',
    'linux-arm64',
    'win32-x64',
    'win32-arm64',
]);
if (!SUPPORTED.has(key)) {
    console.error(`@supercharge-cli/supercharge: unsupported platform ${key}`);
    process.exit(0);
}

// Resolve the per-platform sibling package's directory. The matching
// optionalDependency is installed by npm based on `os`/`cpu` filters; the
// other five are silently skipped. If the matching one is missing, npm was
// likely invoked with --no-optional or the platform is unsupported.
function resolvePlatformPackageDir() {
    const platformPkg = `@supercharge-cli/supercharge-${key}`;
    try {
        return path.dirname(require.resolve(`${platformPkg}/package.json`));
    } catch {
        return null;
    }
}

let version;
try { version = require('../package.json').version; } catch {}
if (!version) {
    console.error('@supercharge-cli/supercharge: unable to determine version');
    process.exit(0);
}

const IS_WINDOWS = process.platform === 'win32';
const EXE = IS_WINDOWS ? '.exe' : '';

fs.mkdirSync(CANONICAL_DIR, { recursive: true });

function writeVendorBinary(brotliPath, binaryPath, destPath) {
    const tmp = destPath + `.tmp.${process.pid}`;
    try {
        if (fs.existsSync(brotliPath)) {
            fs.writeFileSync(tmp, zlib.brotliDecompressSync(fs.readFileSync(brotliPath)));
        } else if (fs.existsSync(binaryPath)) {
            fs.copyFileSync(binaryPath, tmp);
        } else {
            return false;
        }
        if (!IS_WINDOWS) fs.chmodSync(tmp, 0o755);
        fs.renameSync(tmp, destPath);
        return true;
    } catch {
        return false;
    } finally {
        try { fs.unlinkSync(tmp); } catch {}
    }
}

function installBinary(binName, sourceDir, vendorSubpath) {
    const brotliPath = path.join(sourceDir, 'bin', vendorSubpath + '.br');
    const binaryPath = path.join(sourceDir, 'bin', vendorSubpath);

    const versionedName = `${binName}-${version}${EXE}`;
    const versionedPath = path.join(CANONICAL_DIR, versionedName);
    const canonicalName = `${binName}${EXE}`;
    const canonicalPath = path.join(CANONICAL_DIR, canonicalName);

    // Skip if this exact version is already installed.
    if (!fs.existsSync(versionedPath) && !writeVendorBinary(brotliPath, binaryPath, versionedPath)) {
        console.error(`@supercharge-cli/supercharge: missing binary at ${brotliPath}`);
        return false;
    }

    if (IS_WINDOWS) {
        // Symlinks need elevation on Windows; copy instead. If the exe is
        // locked by a running process, rename it aside then retry.
        const oldPath = canonicalPath + '.old';
        try { fs.unlinkSync(oldPath); } catch {} // stale backup from prior update
        try {
            try { fs.unlinkSync(canonicalPath); } catch {}
            fs.copyFileSync(versionedPath, canonicalPath);
        } catch (e) {
            try {
                fs.renameSync(canonicalPath, oldPath);
                try {
                    fs.copyFileSync(versionedPath, canonicalPath);
                } catch (copyErr) {
                    // Rollback: restore the old binary so the install isn't broken.
                    try { fs.renameSync(oldPath, canonicalPath); } catch {}
                    throw copyErr;
                }
            } catch (e2) {
                console.error(`@supercharge-cli/supercharge: failed to update ${canonicalPath}: ${e2.message}`);
                console.error('Close all running supercharge processes and try again.');
                return false;
            }
        }
    } else {
        // Atomic symlink swap.
        const tmpLink = canonicalPath + `.link.${process.pid}`;
        try { fs.unlinkSync(tmpLink); } catch {}
        fs.symlinkSync(versionedName, tmpLink);
        fs.renameSync(tmpLink, canonicalPath);
    }

    // Don't report a broken wire-up as success.
    if (!fs.existsSync(canonicalPath)) {
        console.error(`@supercharge-cli/supercharge: ${canonicalName} did not resolve after install`);
        return false;
    }

    console.log(`${binName} ${version} installed to ${canonicalPath} -> ${versionedName}`);
    return true;
}

// Comparator: sort "<prefix>X.Y.Z" filenames by version, newest first.
function byVersionDescending(prefix) {
    return (a, b) => {
        const pa = a.slice(prefix.length).split('.').map(Number);
        const pb = b.slice(prefix.length).split('.').map(Number);
        for (let i = 0; i < 3; i++) {
            if ((pa[i] || 0) !== (pb[i] || 0)) return (pb[i] || 0) - (pa[i] || 0);
        }
        return 0;
    };
}

// Best-effort cleanup of old versioned binaries for a given binary name.
// Keeps the current version and the previous one (in case a process is still
// running the old binary and hasn't fully loaded all pages yet).
// Uses an exact prefix match + hyphen + digit to avoid supercharge-* matching supercharge-pager-*.
function cleanupOldVersions(binName) {
    try {
        const prefix = `${binName}-`;
        const currentVersioned = `${binName}-${version}${EXE}`;
        const entries = fs.readdirSync(CANONICAL_DIR);
        const versionedBinaries = entries
            .filter(e => {
                if (!e.startsWith(prefix)) return false;
                if (e.includes('.tmp.') || e.includes('.link.')) return false;
                if (e === currentVersioned) return false;
                const suffix = e.slice(prefix.length);
                return /^\d/.test(suffix);
            })
            .sort(byVersionDescending(prefix));
        for (const old of versionedBinaries.slice(1)) {
            try { fs.unlinkSync(path.join(CANONICAL_DIR, old)); } catch {}
        }
    } catch {}
}

const platformDir = resolvePlatformPackageDir();
if (!platformDir) {
    console.error(`@supercharge-cli/supercharge: platform package @supercharge-cli/supercharge-${key} not installed.`);
    console.error('  This usually means npm was invoked with --no-optional, or the install failed.');
    console.error('  Try: npm install -g @supercharge-cli/supercharge');
    process.exit(0);
}

// Point the bin entry at a binary extracted beside it: launches become one
// process, and the link can only dangle if the package itself is broken.
// Windows keeps the node launcher; npm generates its command shims from it.
function installBinLink(platformDir) {
    if (IS_WINDOWS) return;
    // Other package managers wrap the entry's `#!` line in their own launchers.
    if (!(process.env.npm_config_user_agent ?? '').startsWith('npm/')) return;
    const brotliPath = path.join(platformDir, 'bin', `supercharge${EXE}.br`);
    const binaryPath = path.join(platformDir, 'bin', `supercharge${EXE}`);
    const nativePath = path.join(__dirname, 'supercharge-native');
    const entryPath = path.join(__dirname, 'supercharge');
    const tmp = entryPath + `.link.${process.pid}`;
    try {
        if (!writeVendorBinary(brotliPath, binaryPath, nativePath)) {
            return;
        }
        try { fs.unlinkSync(tmp); } catch {}
        fs.symlinkSync('./supercharge-native', tmp);
        fs.renameSync(tmp, entryPath);
    } catch (e) {
        // Losing the link only costs latency; the node launcher still works.
        console.error(`@supercharge-cli/supercharge: bin link not installed: ${e.message}`);
        try { fs.unlinkSync(tmp); } catch {}
    }
}

if (installBinary('supercharge', platformDir, `supercharge${EXE}`)) {
    installBinLink(platformDir);
}
cleanupOldVersions('supercharge');
cleanupOldVersions('supercharge-pager');

// Write installer config
const configDir = SUPERCHARGE_HOME;
const configPath = path.join(configDir, 'config.toml');
let obj = {};
try { obj = TOML.parse(fs.readFileSync(configPath, 'utf8')); } catch { }
obj.cli ??= {};
obj.cli.installer = 'npm';

// Persist the npm registry so `supercharge update` and the launcher use the same one.
const npmRegistry = process.env.SUPERCHARGE_NPM_REGISTRY
    || (() => {
        try {
            const resolved = execSync(
                'npm config get @supercharge-cli:registry',
                { encoding: 'utf8', timeout: 5000 }
            ).trim();
            if (resolved && resolved !== 'undefined') return resolved;
        } catch {}
        return null;
    })();

if (npmRegistry) {
    obj.cli.npm_registry = npmRegistry;
}

fs.writeFileSync(configPath, TOML.stringify(obj), 'utf8');

// Shell completions: print setup hints (no silent shell config mutation).
// Set SUPERCHARGE_INSTALL_COMPLETIONS=1 to auto-generate to ~/.supercharge/completions.
const SUPERCHARGE_PATH = path.join(CANONICAL_DIR, `supercharge${EXE}`);
if (process.env.SUPERCHARGE_INSTALL_COMPLETIONS === '1' && !IS_WINDOWS) {
    try {
        const { spawnSync } = require('child_process');
        const completionsDir = path.join(SUPERCHARGE_HOME, 'completions');
        const bashPath = path.join(completionsDir, 'bash', 'supercharge.bash');
        const zshPath = path.join(completionsDir, 'zsh', '_supercharge');
        fs.mkdirSync(path.dirname(bashPath), { recursive: true });
        fs.mkdirSync(path.dirname(zshPath), { recursive: true });
        const bashRes = spawnSync(SUPERCHARGE_PATH, ['completions', 'bash'], { encoding: 'utf8' });
        if (bashRes.status === 0) fs.writeFileSync(bashPath, bashRes.stdout);
        const zshRes = spawnSync(SUPERCHARGE_PATH, ['completions', 'zsh'], { encoding: 'utf8' });
        if (zshRes.status === 0) fs.writeFileSync(zshPath, zshRes.stdout);
        console.log('Completions generated to ~/.supercharge/completions (bash/zsh)');
    } catch {}
} else if (!IS_WINDOWS) {
    console.log('Tip: supercharge completions bash > ~/.local/share/bash-completion/completions/supercharge');
    console.log('     supercharge completions zsh  > ~/.zsh/completions/_supercharge');
}
