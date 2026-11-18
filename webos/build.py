#!/usr/bin/env python3
"""Build a reviewed GridCraft development browser graph into a new external directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def git(root, *args):
    return subprocess.check_output(['git', '--no-optional-locks', '-C', str(root), *args], text=True).strip()


def external(value, *, provider=False):
    path = Path(value).absolute()
    if '..' in path.parts or any(p.is_symlink() for p in (path, *path.parents)) or path.is_relative_to(ROOT) or ROOT.is_relative_to(path):
        raise ValueError('Direct paths outside source required for outputs and compiler cache')
    if any((p/'.git').exists() or (not provider and (p/'HEAD').is_file() and (p/'objects').is_dir()) for p in (path,*path.parents)):
        raise ValueError('Generated artifacts must be outside all source and bare repositories')
    return path


def verify_inputs(fonts):
    pin=(ROOT/'webos/craft-fonts.rev').read_text().strip()
    if git(fonts,'rev-parse','HEAD')!=pin or git(fonts,'status','--porcelain'):
        raise ValueError('The fonts input must be the clean source-pinned checkout')
    return {'fonts':{'commit':pin}}


def copy_licence_documents(directory, notices, *, declared_licence_directory=False):
    for path in directory.rglob('*'):
        if '.git' in path.relative_to(directory).parts:
            continue
        named_licence = path.suffix.lower() in ('.txt', '.md') and ('license' in path.name.lower() or 'licence' in path.name.lower() or path.name.startswith('OFL'))
        if path.is_file() and (declared_licence_directory or named_licence):
            if path.is_symlink():
                raise ValueError('Regular licence documents required')
            dest = notices/directory.name/path.relative_to(directory)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, dest)


def finalize(output, fonts, tools, *, compiled_source_commit=None):
    html=(output/'index.html').read_text()
    if '<script type="module" src="./bootstrap.mjs"></script>' not in html or '<script>' in html:
        raise ValueError('The hosted graph requires its external CSP-compatible initializer')
    notices = output/'notices'
    notices.mkdir()
    for name in ['LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md']:
        shutil.copy2(ROOT/name, notices/name)
    for directory in [ROOT/'assets/app-icon', fonts]:
        copy_licence_documents(directory, notices, declared_licence_directory=False)
    files = []
    for path in sorted(output.rglob('*')):
        if path.is_symlink():
            raise ValueError('Public artifacts must not refer outside their graph')
        if path.is_file():
            data = path.read_bytes()
            files.append({'path': path.relative_to(output).as_posix(), 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
    if sum(f['path'].endswith('.wasm') for f in files) != 1:
        raise ValueError('The native workbook browser graph must be present')
    packaging_commit = git(ROOT, 'rev-parse', 'HEAD')
    receipt = {'schema': 'webos-component-build/1', 'component': 'gridcraft', 'sourceCommit': compiled_source_commit or packaging_commit,
               'packagingSourceCommit': packaging_commit,
               'profile': 'development', 'tools': tools, 'buildInputs': verify_inputs(fonts),
               'cargoLockSHA256': hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(), 'files': files,
               'totalBytes': sum(f['bytes'] for f in files), 'qualification': 'Built native browser graph; account/browser acceptance is separate'}
    (output/'component-build.json').write_text(json.dumps(receipt, indent=2)+'\n')
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['output', 'target-dir', 'fonts', 'craft-libs-origin', 'egui-origin', 'winit-origin', 'winprint-origin']:
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--jobs', type=int, choices=range(1, 17), default=2)
    args = parser.parse_args()
    output, target = external(args.output), external(args.target_dir)
    if output.exists() or output.is_relative_to(target) or target.is_relative_to(output):
        parser.error('Use a new output directory separate from the one compiler cache')
    if git(ROOT, 'status', '--porcelain'):
        parser.error('Review and commit component changes before creating a reproducible artifact')
    if shutil.disk_usage(output.parent).free < 8*1024**3:
        parser.error('Keep an 8 GiB free-space reserve')
    verify_inputs(args.fonts)
    channel = tomllib.loads((ROOT/'webos/rust-toolchain.toml').read_text())['toolchain']['channel']
    env = dict(os.environ, RUSTUP_TOOLCHAIN=channel, CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS=str(args.jobs),
               CARGO_PROFILE_DEV_DEBUG='0', CARGO_NET_OFFLINE='true', CRAFT_FONTS_REQUIRED='1',
               CRAFT_FONTS_DIR=str(args.fonts))
    for key in list(env):
        if key in ('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_BUILD_RUSTFLAGS','RUSTC_BOOTSTRAP') or key.startswith('CARGO_TARGET_') and key.endswith('_RUSTFLAGS'):
            env.pop(key,None)
    env['RUSTC']=subprocess.check_output(['rustup','which','--toolchain',channel,'rustc'],env=env,text=True).strip()
    env['RUSTDOC']=subprocess.check_output(['rustup','which','--toolchain',channel,'rustdoc'],env=env,text=True).strip()
    env['RUSTC_WRAPPER']=env['RUSTC_WORKSPACE_WRAPPER']=''
    env.setdefault('CARGO_INCREMENTAL','1')
    env.pop('NO_COLOR', None)
    tools = {name: subprocess.check_output([name, '--version'], env=env, text=True).strip() for name in ['cargo', 'wasm-bindgen']}
    tools['rustc']=subprocess.check_output([env['RUSTC'],'--version'],env=env,text=True).strip()
    tools['rustcVerbose']=subprocess.check_output([env['RUSTC'],'--version','--verbose'],env=env,text=True).strip()
    tools['buildConfiguration']={'jobs':args.jobs,'incremental':env['CARGO_INCREMENTAL'],
        'profileOverrides':{key:value for key,value in env.items() if key.startswith('CARGO_PROFILE_')},
        'sourceCargoConfigSHA256':hashlib.sha256((ROOT/'.cargo/config.toml').read_bytes()).hexdigest()}
    if tools['wasm-bindgen'] != 'wasm-bindgen 0.2.129':
        parser.error('The lockfile requires wasm-bindgen 0.2.129')
    # Read exact dependency identities from the lockfile, including upstream's shortened pins.
    sources = {p.get('source', '') for p in tomllib.loads((ROOT/'Cargo.lock').read_text())['package']}
    count = int(env.get('GIT_CONFIG_COUNT', '0'))
    for origin, url in [(args.craft_libs_origin, 'https://github.com/storytold/craft-libs'), (args.egui_origin, 'https://github.com/storytold/egui.git'), (args.winit_origin, 'https://github.com/storytold/winit.git'), (args.winprint_origin, 'https://github.com/storytold/winprint.rs')]:
        origin = external(origin,provider=True)
        if git(origin, 'rev-parse', '--is-bare-repository') != 'true' or (origin/'objects/info/alternates').exists():
            parser.error('A self-contained local dependency provider is required')
        for source in sources:
            if source.startswith('git+'+url+'?'):
                git(origin, 'cat-file', '-e', source.rsplit('#', 1)[1]+'^{commit}')
        env['GIT_CONFIG_KEY_'+str(count)] = 'url.'+origin.as_uri()+'.insteadOf'
        env['GIT_CONFIG_VALUE_'+str(count)] = url
        count += 1
    env['GIT_CONFIG_KEY_'+str(count)], env['GIT_CONFIG_VALUE_'+str(count)] = 'protocol.file.allow', 'always'
    env['GIT_CONFIG_COUNT'] = str(count+1)
    env['CARGO_NET_GIT_FETCH_WITH_CLI'] = 'true'
    commit = git(ROOT, 'rev-parse', 'HEAD')
    os.umask(0o077)
    subprocess.run(['cargo','build','--locked','--offline','-p','gridcraft-web','--target','wasm32-unknown-unknown'],cwd=ROOT,env=env,check=True)
    output.mkdir(mode=0o700)
    subprocess.run(['wasm-bindgen','--target','web','--out-name','gridcraft','--out-dir',str(output),str(target/'wasm32-unknown-unknown/debug/gridcraft-web.wasm')],env=env,check=True)
    for path in output.glob('*.d.ts'):
        path.unlink()
    for name in ['bootstrap.mjs','browser.css']:
        shutil.copy2(ROOT/'webos'/name,output/name)
    shutil.copy2(ROOT/'webos/index.html',output/'index.html')
    if git(ROOT, 'rev-parse', 'HEAD') != commit or git(ROOT, 'status', '--porcelain'):
        raise RuntimeError('Source changed while building; output is unqualified')
    receipt = finalize(output, args.fonts, tools)
    print(json.dumps({'component': 'gridcraft', 'bytes': receipt['totalBytes'], 'files': len(receipt['files'])}))


if __name__ == '__main__':
    main()
