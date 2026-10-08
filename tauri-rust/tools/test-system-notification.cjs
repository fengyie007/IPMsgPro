const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict'),ts=require('typescript');
let invoke,listener,focus;const opened=[],users=[{id:'a'},{id:'b'}],exportsObject={};
const filename=path.join(__dirname,'../src/services/notificationActivation.ts');
vm.runInNewContext(ts.transpileModule(fs.readFileSync(filename,'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText,
 {exports:exportsObject,console,window:{addEventListener:(_,f)=>{focus=f;},removeEventListener:()=>{focus=undefined;}},require(name){
 if(name==='./bridge')return{invoke:(...args)=>invoke(...args),listen:(_,f)=>{listener=f;return()=>{listener=undefined;};}};
 if(name==='../stores/userStore')return{useUserStore:{getState:()=>({users,loadUsers:async()=>{}})}};throw new Error(name);}});
const tick=()=>new Promise(resolve=>setImmediate(resolve));
(async()=>{
 let resolveFirst,count=0;invoke=()=>{count++;return count===1?new Promise(r=>{resolveFirst=r;}):Promise.resolve({userId:'b'});};
 const stop=exportsObject.watchNotificationActivation(user=>opened.push(user.id));listener();listener();
 assert.equal(count,1);resolveFirst({userId:'a'});await tick();assert.deepEqual(opened,['a','b']);assert.equal(count,2);
 console.log('PASS startup and notification clicks serialize activation consumption');
 invoke=async()=>({userId:null});focus();await tick();assert.deepEqual(opened,['a','b']);
 invoke=async()=>({userId:'a'});focus();await tick();assert.deepEqual(opened,['a','b','a']);
 console.log('PASS focus resumes deferred activation without opening an empty target');
 let resolveLate;invoke=()=>new Promise(resolve=>{resolveLate=resolve;});listener();stop();resolveLate({userId:'b'});await tick();assert.equal(opened.length,3);assert.equal(listener,undefined);assert.equal(focus,undefined);
 console.log('PASS disposed notification watchers cannot change conversations');
})().catch(error=>{console.error(error);process.exitCode=1;});
