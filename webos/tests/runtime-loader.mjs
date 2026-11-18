/** Test-only package resolver; provide the independently owned Runtime package explicitly. */
import {readFileSync} from 'node:fs';
import {resolve as pathResolve} from 'node:path';
import {pathToFileURL} from 'node:url';
export function resolve(specifier, context, next) {
  if (specifier.startsWith('@webos/runtime/')) {
    const root=process.env.WEBOS_RUNTIME_SOURCE;
    if(!root)throw Error('WEBOS_RUNTIME_SOURCE must identify the public Runtime package for tests.');
    const manifest=JSON.parse(readFileSync(pathResolve(root,'package.json'),'utf8'));
    const target=manifest.exports?.['./'+specifier.slice('@webos/runtime/'.length)];
    if(typeof target!=='string')throw Error('Runtime does not export '+specifier);
    return {url:pathToFileURL(pathResolve(root,target)).href,shortCircuit:true};
  }
  return next(specifier,context);
}
