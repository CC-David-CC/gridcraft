/** Node 24 test-only resolver for the explicitly supplied public Runtime package. */
import {registerHooks} from 'node:module';
import {resolve} from './runtime-loader.mjs';
registerHooks({resolve});
