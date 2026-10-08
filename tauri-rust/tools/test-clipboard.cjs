const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict'),ts=require('typescript');
let invoke,desktop=true;const exportsObject={};const filename=path.join(__dirname,'../src/services/clipboard.ts');
vm.runInNewContext(ts.transpileModule(fs.readFileSync(filename,'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText,
 {exports:exportsObject,Uint8Array,require(name){if(name==='@tauri-apps/api/core')return{invoke:(...args)=>invoke(...args),isTauri:()=>desktop};throw new Error(name);}});
(async()=>{
 const {clipboardImage,importClipboardImage}=exportsObject;const image=new Blob([new Uint8Array([137,80,78,71])],{type:'image/png'});
 assert.equal(clipboardImage([{type:'text/plain'},image]),image);assert.equal(clipboardImage([{type:'text/plain'}]),null);assert.throws(()=>clipboardImage([image,image]));
 console.log('PASS clipboard image selection does not fetch HTML and rejects multiple images');
 let calls=0;invoke=async(name,bytes)=>{calls++;assert.equal(name,'import_clipboard_image');assert.deepEqual(Array.from(bytes),[137,80,78,71]);return{success:true,image:{assetId:'asset'}};};
 assert.equal((await importClipboardImage(image)).assetId,'asset');assert.equal(calls,1);
 console.log('PASS image bytes use bounded raw IPC and return an asset without sending');
 await assert.rejects(importClipboardImage(new Blob([],{type:'image/png'})));await assert.rejects(importClipboardImage({size:21*1024*1024,type:'image/png'}));
 await assert.rejects(importClipboardImage(new Blob(['x'],{type:'image/svg+xml'})));assert.equal(calls,1);
 console.log('PASS empty, oversized and unsupported clipboard data never invokes native import');
 desktop=false;await assert.rejects(importClipboardImage(image),/桌面版/);desktop=true;invoke=async()=>{throw new Error('bad image');};await assert.rejects(importClipboardImage(image),/bad image/);
 console.log('PASS browser mode and native decoding errors are explicit');
})().catch(e=>{console.error(e);process.exitCode=1;});
