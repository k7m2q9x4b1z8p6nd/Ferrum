(function(g){
if(typeof window==="undefined")g.window=g;
if(typeof document==="undefined"){
var el=function(){return{style:{},dataset:{},children:[],childNodes:[],classList:{add:function(){},remove:function(){},toggle:function(){},contains:function(){return false}},setAttribute:function(){},getAttribute:function(){return null},appendChild:function(c){return c},removeChild:function(c){return c},insertBefore:function(c){return c},remove:function(){},addEventListener:function(){},removeEventListener:function(){},querySelector:function(){return null},querySelectorAll:function(){return[]},getElementsByTagName:function(){return[]}}};
g.document=Object.assign(el(),{readyState:"complete",cookie:"",title:"",head:el(),body:el(),documentElement:el(),createElement:el,createElementNS:el,createTextNode:el,createDocumentFragment:el,getElementById:function(){return null},hasFocus:function(){return false}});
}
var K="rb_localStorage";
function area(persist){
var m=new Map();
function flush(){if(!persist)return;try{var o={};m.forEach(function(v,k){o[k]=v});var d={};d[K]=o;g.chrome.storage.local.set(d)}catch(e){}}
var s={getItem:function(k){k=String(k);return m.has(k)?m.get(k):null},setItem:function(k,v){m.set(String(k),String(v));flush()},removeItem:function(k){m.delete(String(k));flush()},clear:function(){m.clear();flush()},key:function(i){var a=Array.from(m.keys());return i<a.length?a[i]:null}};
Object.defineProperty(s,"length",{get:function(){return m.size}});
if(persist){try{g.chrome.storage.local.get(K,function(r){var o=r&&r[K];if(o)Object.keys(o).forEach(function(k){if(!m.has(k))m.set(k,o[k])})})}catch(e){}}
return s;
}
if(typeof localStorage==="undefined")g.localStorage=area(true);
if(typeof sessionStorage==="undefined")g.sessionStorage=area(false);
if(typeof XMLHttpRequest==="undefined"){
var X=function(){this.readyState=0;this.status=0;this.statusText="";this.responseText="";this.response=null;this.responseType="";this.responseURL="";this.timeout=0;this._h={};this._l={}};
var P=X.prototype;
P._fire=function(t){var e={type:t,target:this},f=this["on"+t];if(f)f.call(this,e);(this._l[t]||[]).forEach(function(h){h.call(this,e)},this)};
P.open=function(m,u){this._m=String(m).toUpperCase();this._u=String(u);this.readyState=1;this._fire("readystatechange")};
P.setRequestHeader=function(k,v){this._h[k]=v};
P.getResponseHeader=function(k){return this._r?this._r.headers.get(k):null};
P.getAllResponseHeaders=function(){var s="";if(this._r)this._r.headers.forEach(function(v,k){s+=k+": "+v+"\r\n"});return s};
P.overrideMimeType=function(){};
P.addEventListener=function(t,f){(this._l[t]=this._l[t]||[]).push(f)};
P.removeEventListener=function(t,f){this._l[t]=(this._l[t]||[]).filter(function(h){return h!==f})};
P.abort=function(){this._x=1;this._fire("abort")};
P.send=function(b){
var x=this;
fetch(x._u,{method:x._m,headers:x._h,body:/^(GET|HEAD)$/.test(x._m)?undefined:b}).then(function(r){
x._r=r;x.status=r.status;x.statusText=r.statusText;x.responseURL=r.url;x.readyState=2;x._fire("readystatechange");
return x.responseType==="arraybuffer"?r.arrayBuffer():x.responseType==="blob"?r.blob():r.text();
}).then(function(t){
if(x._x)return;
x.readyState=4;
if(typeof t==="string"){x.responseText=t;x.response=x.responseType==="json"?JSON.parse(t):t}else x.response=t;
x._fire("readystatechange");x._fire("load");x._fire("loadend");
}).catch(function(){
if(x._x)return;
x.readyState=4;x._fire("readystatechange");x._fire("error");x._fire("loadend");
});
};
g.XMLHttpRequest=X;
}
})(globalThis);
