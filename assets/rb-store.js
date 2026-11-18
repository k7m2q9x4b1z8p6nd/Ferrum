(function(){
if(top!==window)return;
var h=location.hostname,c=h=="chromewebstore.google.com"||h=="chrome.google.com",a=h=="addons.mozilla.org";
if(!c&&!a)return;
var L="Add to Rust Browser",t,el;
function find(){
 var p=location.pathname,m;
 if(c){m=p.match(/\/([a-p]{32})(?:\/|$)/);return m&&{k:"chrome",id:m[1]}}
 m=p.match(/\/addon\/([^\/]+)/);return m&&{k:"amo",id:decodeURIComponent(m[1])}
}
function btn(n){
 var b=n&&n.closest&&n.closest(c?"button":"a.GetFirefoxButton-button");
 return b&&find()&&(a||/UywwFc-LgbsSe/.test(b.className)||/add to chrome/i.test(b.textContent))?b:null
}
function label(b){
 var w=document.createTreeWalker(b,4),n;
 while(n=w.nextNode())if(n.nodeValue.trim()){if(n.nodeValue.trim()!=L)n.nodeValue=L;return}
}
document.addEventListener("click",function(e){
 var b=btn(e.target),r=b&&find();
 if(!r)return;
 e.preventDefault();e.stopImmediatePropagation();
 window.ipc.postMessage(JSON.stringify({c:"store_install",k:r.k,id:r.id}))
},true);
setInterval(function(){
 if(find())document.querySelectorAll(c?"button":"a.GetFirefoxButton-button").forEach(function(x){if(btn(x))label(x)})
},700);
window.rbExtStatus=function(r){
 if(r.s=="idle"){if(el){el.remove();el=null}return}
 if(!el){
  el=document.createElement("div");
  el.style.cssText="position:fixed;right:16px;bottom:16px;z-index:2147483647;max-width:360px;padding:10px 14px;border-radius:8px;font:13px system-ui,sans-serif;color:#fff;box-shadow:0 4px 16px rgba(0,0,0,.4)";
  document.documentElement.append(el)
 }
 el.style.background=r.s=="err"?"#b3261e":r.s=="ok"?"#1e7a46":"#333";
 el.textContent=r.m+(r.s=="busy"?"…":"");
 clearTimeout(t);
 if(r.s!="busy")t=setTimeout(function(){if(el){el.remove();el=null}},r.s=="ok"?4000:8000)
};
})();
