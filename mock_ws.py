#!/usr/bin/env python3
"""
mock_ws.py — Mock WebSocket server for testing the Guardian PWA UI.

Run:  python3 mock_ws.py
Then:  cd pwa-wasm && trunk serve
Open:  http://localhost:8080

This simulates the firmware's WebSocket on port 81:
  - Sends telemetry 10×/sec (db, armed, tripwire, ducking, fw, pwa)
  - Responds to all PWA commands (arm, disarm, calibrate, set_tv, etc.)
  - Simulates wifi_scan, discover_tvs, ota_check events
"""

import asyncio
import json
import math
import time

try:
    import websockets
except ImportError:
    print("Install websockets: pip install websockets")
    raise SystemExit(1)

# ── State ────────────────────────────────────────────────────────────────────

state = {
    "armed": False,
    "tripwire": -20.0,
    "floor": -60.0,
    "ducking": False,
    "fw": "0.3.0",
    "pwa": "0.1.0",
    "tv_ip": "",
    "tv_brand": "",
}

t = 0  # time counter for sine wave

# ── Mock data ────────────────────────────────────────────────────────────────

MOCK_NETWORKS = [
    {"ssid": "HomeNetwork_5G", "rssi": -45},
    {"ssid": "HomeNetwork_2G", "rssi": -52},
    {"ssid": "Neighbor_WiFi", "rssi": -72},
    {"ssid": "CoffeeShop", "rssi": -81},
]

MOCK_TVS = [
    {"ip": "192.168.1.100", "name": "LG WebOS TV", "brand": "lg"},
    {"ip": "192.168.1.101", "name": "Samsung Smart TV", "brand": "samsung"},
    {"ip": "192.168.1.102", "name": "Roku Ultra", "brand": "roku"},
]

# ── Ducking simulation ───────────────────────────────────────────────────────

sustained_ms = 0
duck_hold_start = 0

def make_telemetry():
    global t, sustained_ms, duck_hold_start
    t += 1
    # Sine wave dB — simulates normal TV with occasional loud scenes
    # Loud "scene" every ~30s, lasts ~10s
    cycle = t % 300
    if 100 < cycle < 200:
        # Loud scene
        base = -15.0 + 5.0 * math.sin(t * 0.3)
    else:
        # Normal TV
        base = -35.0 + 5.0 * math.sin(t * 0.1)

    if state["armed"]:
        if base > state["tripwire"]:
            sustained_ms = min(sustained_ms + 100, 10000)
        else:
            sustained_ms = max(sustained_ms - 50, 0)

        if sustained_ms >= 3000 and not state["ducking"]:
            state["ducking"] = True
            duck_hold_start = t
            print(f"[mock] Ducking started (sustained={sustained_ms}ms)")

        if state["ducking"]:
            # Path A: near-silence restore
            if base < state["floor"] + 2.0:
                state["ducking"] = False
                sustained_ms = 0
                print("[mock] Restored (near-silence)")
            # Path B: sustained quiet + 30s hold
            elif sustained_ms == 0 and (t - duck_hold_start) > 300:
                state["ducking"] = False
                print("[mock] Restored (30s hold elapsed)")
    else:
        sustained_ms = 0
        state["ducking"] = False

    return json.dumps({
        "db": round(base, 2),
        "armed": state["armed"],
        "tripwire": round(state["tripwire"], 1),
        "ducking": state["ducking"],
        "fw": state["fw"],
        "pwa": state["pwa"],
    })

# ── Command handler ──────────────────────────────────────────────────────────

async def handle_command(ws, text):
    try:
        msg = json.loads(text)
    except json.JSONDecodeError:
        return

    cmd = msg.get("cmd", "")

    if cmd == "arm":
        state["armed"] = True
        print("[mock] Armed")

    elif cmd == "disarm":
        state["armed"] = False
        state["ducking"] = False
        print("[mock] Disarmed")

    elif cmd == "calibrate_silence":
        db = msg.get("db", -60.0)
        state["floor"] = db
        print(f"[mock] Floor set to {db:.1f} dBFS")

    elif cmd == "calibrate_max":
        db = msg.get("db", -20.0)
        state["tripwire"] = db - 3.0
        print(f"[mock] Tripwire set to {db - 3.0:.1f} dBFS")

    elif "threshold" in msg:
        state["tripwire"] = msg["threshold"]
        print(f"[mock] Manual tripwire: {msg['threshold']:.1f}")

    elif cmd == "scan_wifi":
        print("[mock] WiFi scan — returning mock networks")
        await asyncio.sleep(1.5)  # simulate scan delay
        await ws.send(json.dumps({
            "evt": "wifi_scan",
            "networks": MOCK_NETWORKS,
        }))

    elif cmd == "set_wifi":
        ssid = msg.get("ssid", "?")
        print(f"[mock] WiFi reconfigure → {ssid}")
        await ws.send(json.dumps({
            "evt": "wifi_reconfiguring",
            "ssid": ssid,
        }))
        # In real firmware this would reboot

    elif cmd == "set_tv":
        ip = msg.get("ip", "")
        brand = msg.get("brand", "")
        state["tv_ip"] = ip
        state["tv_brand"] = brand
        if ip:
            print(f"[mock] TV connected: {brand} @ {ip}")
        else:
            print("[mock] TV disconnected")

    elif cmd == "discover_tvs":
        print("[mock] SSDP discover — returning mock TVs")
        await asyncio.sleep(2.0)  # simulate 3s SSDP scan
        await ws.send(json.dumps({
            "evt": "discovered",
            "tvs": MOCK_TVS,
        }))

    elif cmd == "ota_check":
        print("[mock] OTA check — returning up-to-date")
        await asyncio.sleep(1.0)
        await ws.send(json.dumps({
            "evt": "ota_status",
            "checking": False,
            "available": False,
            "current": state["pwa"],
            "latest": state["pwa"],
            "fw": state["fw"],
        }))

    else:
        print(f"[mock] Unknown command: {text}")

# ── WebSocket server ─────────────────────────────────────────────────────────

async def serve(ws):
    print(f"[mock] Client connected from {ws.remote_address}")

    async def sender():
        while True:
            try:
                await ws.send(make_telemetry())
            except websockets.ConnectionClosed:
                break
            await asyncio.sleep(0.1)

    async def receiver():
        async for msg in ws:
            await handle_command(ws, msg)

    await asyncio.gather(sender(), receiver())
    print("[mock] Client disconnected")

async def main():
    print("Guardian Mock WS Server")
    print("━━━━━━━━━━━━━━━━━━━━━━━")
    print("Listening on ws://localhost:81")
    print("Run 'cd pwa-wasm && trunk serve' then open http://localhost:8080")
    print()

    async with websockets.serve(serve, "0.0.0.0", 81):
        await asyncio.Future()  # run forever

if __name__ == "__main__":
    asyncio.run(main())
