import json
import time
import urllib.request

URL = "http://127.0.0.1:47800/mcp"

def call_mcp_tool(tool_name, arguments=None):
    if arguments is None: arguments = {}
    payload = {
        "jsonrpc": "2.0",
        "id": int(time.time() * 1000),
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments}
    }
    req = urllib.request.Request(URL, data=json.dumps(payload).encode("utf-8"), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read().decode("utf-8"))
        content = data.get("result", {}).get("content", [])
        for item in content:
            if item.get("type") == "text":
                try: return json.loads(item.get("text"))
                except: return item.get("text")
        return data.get("result")

def main():
    print("Disconnecting any previous session...")
    try: call_mcp_tool("disconnect")
    except: pass
    time.sleep(1)
    print("Connecting...")
    print("connect:", call_mcp_tool("connect", {"host": "127.0.0.1", "port": 25566}))
    time.sleep(3)

    st = call_mcp_tool("get_state")
    print("Initial state:", json.dumps(st, indent=2))

    print("\n--- Direct get_block queries at spawn (0, Y, 0) ---")
    for y in range(48, 55):
        blk = call_mcp_tool("get_block", {"x": 0, "y": y, "z": 0})
        print(f"  (0, {y}, 0) -> {blk}")

    # Query area around spawn
    area = call_mcp_tool("get_area", {"min": [-2, 45, -2], "max": [2, 55, 2]})
    non_air_blocks = []
    if isinstance(area, list):
        for b in area:
            block_str = str(b.get("block", ""))
            if "air" not in block_str.lower():
                non_air_blocks.append(b)

    print(f"\nNon-air blocks in 5x11x5 area around spawn: count={len(non_air_blocks)}")
    for b in non_air_blocks[:15]:
        print(f"  ({b.get('x')}, {b.get('y')}, {b.get('z')}): {b.get('block')}")

    # Move to X=32 (Chunk 2)
    call_mcp_tool("run_script", {"steps": [{"ticks": 120, "keys": ["forward"], "yaw": -90.0, "pitch": 0.0}]})
    time.sleep(4)

    st2 = call_mcp_tool("get_state")
    print("\nState after movement:", json.dumps(st2, indent=2))

    rec2 = st2.get("latest_tick_record", {})
    pos = rec2.get("pos", [25, 52, 0])
    px, py, pz = int(pos[0]), int(pos[1]), int(pos[2])

    print(f"\n--- Direct get_block queries near position ({px}, Y, {pz}) ---")
    for y in range(48, 55):
        blk = call_mcp_tool("get_block", {"x": px, "y": y, "z": pz})
        print(f"  ({px}, {y}, {pz}) -> {blk}")

    area_c2 = call_mcp_tool("get_area", {"min": [px-2, 45, pz-2], "max": [px+2, 55, pz+2]})
    non_air_c2 = []
    if isinstance(area_c2, list):
        for b in area_c2:
            block_str = str(b.get("block", ""))
            if "air" not in block_str.lower():
                non_air_c2.append(b)

    print(f"\nNon-air blocks near position ({px}, Y, {pz}): count={len(non_air_c2)}")
    for b in non_air_c2[:15]:
        print(f"  ({b.get('x')}, {b.get('y')}, {b.get('z')}): {b.get('block')}")

    call_mcp_tool("disconnect")

if __name__ == "__main__":
    main()
