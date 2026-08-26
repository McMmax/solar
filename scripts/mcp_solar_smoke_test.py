import json
import os
import sys
import time
import urllib.request

URL = "http://127.0.0.1:47800/mcp"
LOG_DIR = os.path.join(os.path.dirname(__file__), ".logs")
os.makedirs(LOG_DIR, exist_ok=True)
LOG_FILE = os.path.join(LOG_DIR, f"mcp_solar_smoke_test_{int(time.time())}.log")

def log(msg):
    timestamp = time.strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{timestamp}] {msg}"
    print(line)
    with open(LOG_FILE, "a", encoding="utf-8") as f:
        f.write(line + "\n")

def call_mcp_tool(tool_name, arguments=None):
    if arguments is None:
        arguments = {}
    payload = {
        "jsonrpc": "2.0",
        "id": int(time.time() * 1000),
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments}
    }
    req = urllib.request.Request(
        URL,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read().decode("utf-8"))
        content = data.get("result", {}).get("content", [])
        for item in content:
            if item.get("type") == "text":
                try:
                    return json.loads(item.get("text"))
                except Exception:
                    return item.get("text")
        return data.get("result")

def main():
    log("=== Starting Solar MCP Terrain Smoke Test ===")
    
    log("Step 1: Disconnecting any existing session...")
    try:
        call_mcp_tool("disconnect")
    except Exception as e:
        log(f"Disconnect attempt note: {e}")
    time.sleep(1)

    log("Step 2: Connecting to Stomzy Solar server (127.0.0.1:25566)...")
    res = call_mcp_tool("connect", {"host": "127.0.0.1", "port": 25566})
    log(f"Connect result: {res}")
    time.sleep(3)

    st = call_mcp_tool("get_state")
    if not isinstance(st, dict) or not st.get("connected"):
        log("ERROR: MCPClient failed to connect to Stomzy server.")
        sys.exit(1)

    log(f"Connected state: tick={st.get('tick')}, pos=({st.get('x')}, {st.get('y')}, {st.get('z')}), on_ground={st.get('on_ground')}")

    log("Step 3: Probing spawn terrain blocks at (0, Y, 0)...")
    spawn_blocks = {}
    non_air_spawn = []
    for y in range(45, 55):
        blk = call_mcp_tool("get_block", {"x": 0, "y": y, "z": 0})
        bname = blk.get("block", "") if isinstance(blk, dict) else str(blk)
        spawn_blocks[y] = bname
        log(f"  Y={y}: {bname}")
        if "air" not in bname.lower() and bname != "":
            non_air_spawn.append((y, bname))

    if not non_air_spawn:
        log("FAIL: No non-air blocks found at spawn (0, Y, 0)! Solar terrain did not load.")
        sys.exit(1)

    log(f"SUCCESS: Found {len(non_air_spawn)} non-air blocks at spawn (0, Y, 0): {non_air_spawn}")

    log("Step 4: Executing player movement script across chunk boundary (moving +X)...")
    call_mcp_tool("run_script", {"steps": [{"ticks": 100, "keys": ["forward"], "yaw": -90.0, "pitch": 0.0}]})
    time.sleep(3)

    st2 = call_mcp_tool("get_state")
    rec2 = st2.get("latest_tick_record", {}) if isinstance(st2, dict) else {}
    pos2 = rec2.get("pos", [st2.get("x", 0), st2.get("y", 0), st2.get("z", 0)])
    px, py, pz = int(pos2[0]), int(pos2[1]), int(pos2[2])
    on_ground2 = st2.get("on_ground")
    block_below2 = rec2.get("block_below")

    log(f"Moved player state: pos=({pos2[0]:.2f}, {pos2[1]:.2f}, {pos2[2]:.2f}), on_ground={on_ground2}, block_below={block_below2}")

    log(f"Step 5: Probing Chunk 1/2 terrain blocks at ({px}, Y, {pz})...")
    c2_blocks = {}
    non_air_c2 = []
    for y in range(45, 55):
        blk = call_mcp_tool("get_block", {"x": px, "y": y, "z": pz})
        bname = blk.get("block", "") if isinstance(blk, dict) else str(blk)
        c2_blocks[y] = bname
        log(f"  Y={y}: {bname}")
        if "air" not in bname.lower() and bname != "":
            non_air_c2.append((y, bname))

    if not non_air_c2:
        log(f"FAIL: No non-air blocks found across chunk boundary ({px}, Y, {pz})!")
        sys.exit(1)

    log(f"SUCCESS: Found {len(non_air_c2)} non-air blocks across chunk boundary ({px}, Y, {pz}): {non_air_c2}")

    log("Step 6: Disconnecting session...")
    call_mcp_tool("disconnect")

    log("=== Solar MCP Terrain Smoke Test PASSED ===")
    sys.exit(0)

if __name__ == "__main__":
    main()
