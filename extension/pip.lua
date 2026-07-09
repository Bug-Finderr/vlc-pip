-- capabilities={"trigger"}: VLC calls trigger() on every menu click (no checkmark state).

function descriptor()
    return {
        title = "PiP Mode",
        version = "2.1.2",
        author = "Sudharsan",
        shortdesc = "PiP Mode",
        description = "Toggle VLC into a borderless always-on-top corner window",
        capabilities = { "trigger" },
    }
end

-- Keep env lookups lazy: any top-level error silently drops the extension from the View menu (SPEC gotcha #2).
local function temp_dir()
    return (os.getenv("TEMP") or os.getenv("TMP") or ".")
end

local function appdata_dir()
    return (os.getenv("APPDATA") or ".")
end

local function daemon_alive()
    local f = io.open(temp_dir() .. "\\vlc-pip-daemon.alive", "r")
    if not f then return false end
    local ts = f:read("*n")
    f:close()
    -- existence alone is not liveness (a force-kill leaves the file); ts == nil = read mid-write, the daemon IS alive, never respawn.
    return ts == nil or math.abs(os.time() - ts) < 15
end

local function write_request(cmd)
    local f, e = io.open(temp_dir() .. "\\vlc-pip-request.txt", "w")
    if not f then error("cannot write request file: " .. tostring(e)) end
    f:write(cmd)
    f:close()
end

local function ensure_daemon()
    if daemon_alive() then return end
    local exe = appdata_dir() .. "\\vlc\\pip\\pip-helper.exe"
    local p = io.open(exe, "rb")
    if not p then error("pip-helper.exe missing at " .. exe .. " - run scripts\\install.ps1") end
    p:close()
    -- Fallback only (may flash a console once). Normally the daemon starts at login.
    os.execute('start "" "' .. exe .. '" daemon')
end

function trigger()
    local ok, err = pcall(function()
        ensure_daemon()
        write_request("toggle")
    end)
    if not ok then vlc.msg.err("pip: " .. tostring(err)) end
end
