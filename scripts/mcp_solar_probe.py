import json
import time
import urllib.request
import sys

URL = "http://127.0.0.1:47800/mcp"

def call_mcp_tool(tool_name, arguments=None):
    if arguments is None:
        arguments = {}
    payload = {
        "jsonrpc": "2.0",
        "id": int(time.time() * 1000),
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        }
    }
    req = urllib.request.Request(
        URL,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if "error" in data:
                print(f"[MCP ERROR] {tool_name}: {data['error']}", file=sys.stderr)
                return data
            content = data.get("result", {}).get("content", [])
            for item in content:
                if item.get("type") == "text":
                    try:
                        return json.loads(item.get("text"))
                    except Exception:
                        return item.get("text")
            return data.get("result")
    except Exception as e:
        print(f"[HTTP ERROR] {tool_name}: {e}", file=sys.stderr)
        return {"error": str(e)}

def main():
    print("=== Step 1: Start Capture ===")
    r = call_mcp_tool("capture_start", {"label": "solar-terrain-probe"})
    print("capture_start:", json.dumps(r, indent=2))

    print("\n=== Step 2: Connect to localhost:25566 ===")
    r = call_mcp_tool("connect", {"host": "127.0.0.1", "port": 25566})
    print("connect:", json.dumps(r, indent=2))

    print("\n=== Waiting 3 seconds for handshake & initial chunk loading ===")
    time.sleep(3)

    print("\n=== Step 3: Get State ===")
    state1 = call_mcp_tool("get_state")
    print("get_state (spawn):", json.dumps(state1, indent=2))

    print("\n=== Step 4: Get Block & Area at Spawn ===")
    block_spawn = call_mcp_tool("get_block", {"x": 0, "y": 64, "z": 0})
    print("get_block (0, 64, 0):", json.dumps(block_spawn, indent=2))

    area_spawn = call_mcp_tool("get_area", {"min": [-2, 60, -2], "max": [2, 66, 2]})
    print("get_area spawn bounds:", json.dumps(area_spawn, indent=2))

    print("\n=== Step 5: Move Forward Across Chunk Boundary ===")
    script = {
        "steps": [
            {"ticks": 40, "keys": ["forward"], "yaw": 0.0, "pitch": 0.0}
        ]
    }
    r_script = call_mcp_tool("run_script", script)
    print("run_script:", json.dumps(r_script, indent=2))

    print("\n=== Waiting 3 seconds for movement & next chunk load ===")
    time.sleep(3)

    print("\n=== Step 6: Get State after Movement ===")
    state2 = call_mcp_tool("get_state")
    print("get_state (chunk 2):", json.dumps(state2, indent=2))

    px = 0
    py = 64
    pz = 0
    if isinstance(state2, dict) and "position" in state2:
        pos = state2["position"]
        if isinstance(pos, list) and len(pos) >= 3:
            px = int(pos[0])
            py = int(pos[1])
            pz = int(pos[2])

    print(f"\n=== Step 7: Get Block & Area in Second Chunk around ({px}, {py}, {pz}) ===")
    block_c2 = call_mcp_tool("get_block", {"x": px, "y": py, "z": pz})
    print(f"get_block ({px}, {py}, {pz}):", json.dumps(block_c2, indent=2))

    area_c2 = call_mcp_tool("get_area", {"min": [px - 2, py - 2, pz - 2], "max": [px + 2, py + 2, pz + 2]})
    print("get_area second chunk:", json.dumps(area_c2, indent=2))

    print("\n=== Step 8: Stop Capture ===")
    r_cap_stop = call_mcp_tool("capture_stop")
    print("capture_stop:", json.dumps(r_cap_stop, indent=2))

    print("\n=== Step 9: Disconnect cleanly ===")
    r_disc = call_mcp_tool("disconnect")
    print("disconnect:", json.dumps(r_disc, indent=2))

if __name__ == "__main__":
    main()
