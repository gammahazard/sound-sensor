//! setup_html.rs — Self-contained HTML setup page for AP mode WiFi provisioning
//!
//! Served when the device is in AP mode (no WiFi creds in flash).
//! Connects via WebSocket to port 81 and reuses existing scan_wifi / set_wifi commands.
//!
//! Flow:
//!   1. Page loads, connects WS to Guardian
//!   2. Auto-scans for networks on connect (cyw43 goes off-channel ~3s, WS drops)
//!   3. WS auto-reconnects, scan results arrive, network list appears
//!   4. User taps their network, enters password, taps Connect
//!   5. Guardian saves creds to flash and reboots into station mode

pub const SETUP_PAGE: &[u8] = br##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,user-scalable=no">
<title>Guardian Setup</title>
<style>
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:-apple-system,system-ui,sans-serif;background:#0f172a;color:#f1f5f9;
     display:flex;justify-content:center;min-height:100vh;padding:16px;font-size:16px}
.wrap{max-width:400px;width:100%;display:flex;flex-direction:column;gap:16px;padding-top:24px}
h1{text-align:center;font-size:24px;font-weight:700}
.sub{text-align:center;color:#94a3b8;font-size:14px;line-height:1.4}
.card{background:#1e293b;border-radius:16px;padding:16px;display:flex;flex-direction:column;gap:12px}
label{font-size:12px;color:#94a3b8}
input{background:#0f172a;border:1px solid #334155;border-radius:10px;padding:12px;
      color:#f1f5f9;font-size:16px;width:100%;-webkit-appearance:none}
.btn{width:100%;padding:14px;border-radius:12px;border:none;font-size:16px;font-weight:600;
     cursor:pointer;-webkit-appearance:none}
.btn-primary{background:#6366f1;color:#fff}
.btn-primary:disabled{opacity:0.5}
.btn-scan{background:transparent;border:1px solid #475569;color:#93c5fd}
.net{text-align:left;width:100%;padding:12px;border-radius:10px;border:1px solid #334155;
     background:#0f172a;color:#f1f5f9;cursor:pointer;display:flex;justify-content:space-between;
     align-items:center;font-size:15px;-webkit-appearance:none}
.net:hover,.net:active{border-color:#6366f1;background:#1e293b}
.bars{color:#94a3b8;font-size:11px;letter-spacing:1px}
.status{text-align:center;font-size:13px;min-height:20px;font-weight:500;line-height:1.4}
.ok{color:#86efac}.err{color:#fca5a5}.info{color:#fde68a}
.spinner{display:inline-block;width:14px;height:14px;border:2px solid #475569;
         border-top-color:#93c5fd;border-radius:50%;animation:spin .8s linear infinite;
         vertical-align:middle;margin-right:6px}
@keyframes spin{to{transform:rotate(360deg)}}
#passRow{display:none}
</style>
</head>
<body>
<div class="wrap">
  <h1>Guardian Setup</h1>
  <p class="sub">Connect your Guardian sound sensor to WiFi</p>
  <div class="card">
    <div class="status" id="status"><span class="spinner"></span>Connecting to Guardian...</div>
    <button class="btn btn-scan" id="scanBtn" onclick="doScan()">Scan for Networks</button>
    <div id="nets"></div>
    <div id="passRow">
      <label>Password for <strong id="selectedNet"></strong></label>
      <input id="pass" type="password" placeholder="WiFi password" autocomplete="off">
    </div>
    <div id="manualRow" style="display:none">
      <label>Network Name</label>
      <input id="ssid" type="text" placeholder="Enter WiFi name manually">
    </div>
    <button class="btn btn-primary" id="connectBtn" onclick="doConnect()" style="display:none">Connect</button>
    <button class="btn btn-scan" id="manualBtn" onclick="showManual()" style="display:none">Enter network name manually</button>
  </div>
</div>
<script>
var ws,connected=false,scanning=false,pendingScan=false,selectedSsid='',gotResults=false,done=false;
function esc(s){return s.replace(/\\/g,'\\\\').replace(/"/g,'\\"')}
function html(s){return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;')}
function $(id){return document.getElementById(id)}

function init(){
  var host=location.hostname||'192.168.4.1';
  try{ws=new WebSocket('ws://'+host+':81/ws')}catch(e){setTimeout(init,2000);return}
  ws.onopen=function(){
    connected=true;
    if(done)return;
    if(pendingScan){pendingScan=false;doScan()}
    else if(!gotResults){stat('Connected! Scanning for networks...','info');doScan()}
    else{stat('Connected','ok')}
  };
  ws.onclose=function(){
    connected=false;
    if(done)return;
    if(scanning){stat('<span class="spinner"></span>Scanning nearby networks...','info')}
    else{stat('<span class="spinner"></span>Reconnecting to Guardian...','info')}
    setTimeout(init,1500);
  };
  ws.onerror=function(){};
  ws.onmessage=function(e){
    try{var m=JSON.parse(e.data);
      if(m.evt==='wifi_scan'&&m.networks){scanning=false;showNets(m.networks)}
      if(m.evt==='wifi_reconfiguring'){
        done=true;
        var ssidName=selectedSsid||$('ssid').value.trim()||'your WiFi';
        $('status').innerHTML='<div style="text-align:left;line-height:1.8">'
          +'<div style="color:#86efac;font-weight:600;margin-bottom:8px">WiFi credentials saved! Guardian is restarting...</div>'
          +'<div style="color:#94a3b8;font-size:13px;margin-bottom:4px"><strong style="color:#f1f5f9">What to do next:</strong></div>'
          +'<div style="color:#94a3b8;font-size:13px">1. Close this page</div>'
          +'<div style="color:#94a3b8;font-size:13px">2. Reconnect your phone to <strong style="color:#f1f5f9">'+html(ssidName)+'</strong></div>'
          +'<div style="color:#94a3b8;font-size:13px">3. Open <strong style="color:#93c5fd">guardian.local</strong> in your browser</div>'
          +'<div style="color:#94a3b8;font-size:13px;margin-top:8px">Your Guardian dashboard will be waiting for you.</div>'
          +'</div>';
        $('status').className='status';
        $('connectBtn').disabled=true;
        $('connectBtn').textContent='Restarting...';
      }
    }catch(x){}
  };
}

function stat(msg,cls){var el=$('status');el.innerHTML=msg;el.className='status '+(cls||'')}

function doScan(){
  if(!connected){pendingScan=true;return}
  scanning=true;
  $('scanBtn').innerHTML='<span class="spinner"></span>Scanning...';
  ws.send('{"cmd":"scan_wifi"}');
  setTimeout(function(){
    if(scanning){$('scanBtn').textContent='Scan for Networks';scanning=false}
  },12000);
}

function showNets(nets){
  gotResults=true;
  var h='';
  nets=nets.filter(function(n){return n.ssid&&n.ssid!=='Guardian-Setup'});
  nets.sort(function(a,b){return b.rssi-a.rssi});
  if(nets.length===0){
    stat('No networks found. Move closer to your router and try again.','err');
    $('scanBtn').textContent='Scan Again';
    $('manualBtn').style.display='block';
    return;
  }
  for(var i=0;i<nets.length;i++){
    var r=nets[i].rssi;
    var bars=r>-50?'\u2587\u2587\u2587\u2587':r>-60?'\u2587\u2587\u2587\u2581':r>-70?'\u2587\u2587\u2581\u2581':'\u2587\u2581\u2581\u2581';
    h+='<button class="net" onclick="pickNet(\''+html(esc(nets[i].ssid))+'\')">'
      +'<span>'+html(nets[i].ssid)+'</span><span class="bars">'+bars+'</span></button>';
  }
  $('nets').innerHTML=h;
  $('scanBtn').textContent='Scan Again';
  $('manualBtn').style.display='block';
  stat('Select your WiFi network','info');
}

function pickNet(ssid){
  selectedSsid=ssid;
  $('selectedNet').textContent=ssid;
  $('passRow').style.display='block';
  $('manualRow').style.display='none';
  $('connectBtn').style.display='block';
  $('connectBtn').disabled=false;
  $('connectBtn').textContent='Connect';
  $('pass').value='';
  $('pass').focus();
  stat('Enter the password for '+html(ssid),'info');
}

function showManual(){
  $('manualRow').style.display='block';
  $('passRow').style.display='block';
  $('selectedNet').textContent='manual network';
  $('connectBtn').style.display='block';
  $('connectBtn').disabled=false;
  $('connectBtn').textContent='Connect';
  $('ssid').focus();
  selectedSsid='';
  stat('Enter your network name and password','info');
}

function doConnect(){
  var ssid=selectedSsid||$('ssid').value.trim();
  var pass=$('pass').value;
  if(!ssid){stat('Select or enter a network name','err');return}
  if(!connected){stat('Not connected to Guardian. Reconnecting...','err');return}
  $('connectBtn').disabled=true;
  $('connectBtn').innerHTML='<span class="spinner"></span>Connecting...';
  ws.send('{"cmd":"set_wifi","ssid":"'+esc(ssid)+'","pass":"'+esc(pass)+'"}');
  stat('Saving credentials... Guardian will reboot in a few seconds.','info');
}

init();
</script>
</body>
</html>"##;
