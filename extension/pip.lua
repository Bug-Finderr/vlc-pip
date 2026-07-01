-- VLC PiP: View-menu trigger that toggles the PiP daemon.
-- capabilities={"trigger"}: VLC calls trigger() on every menu click (no checkmark state).

function descriptor()
    return {
        title = "PiP Mode",
        version = "1.0.0",
        author = "Sudharsan",
        shortdesc = "PiP Mode",
        description = "Toggle VLC into a borderless always-on-top corner window",
        capabilities = { "trigger" },
    }
end

local function temp_dir()
    return (os.getenv("TEMP") or os.getenv("TMP") or ".")
end

local function appdata_dir()
    return (os.getenv("APPDATA") or ".")
end

local function daemon_alive()
    local f = io.open(temp_dir() .. "\\vlc-pip-daemon.alive", "r")
    if f then f:close() return true end
    return false
end

local function write_request(cmd)
    local f = io.open(temp_dir() .. "\\vlc-pip-request.txt", "w")
    if f then f:write(cmd) f:close() end
end

local function ensure_daemon()
    if daemon_alive() then return end
    -- Fallback only (may flash a console once). Normally the daemon starts at login.
    local exe = appdata_dir() .. "\\vlc\\pip\\pip-helper.exe"
    os.execute('start "" "' .. exe .. '" daemon')
end

function trigger()
    local ok, err = pcall(function()
        ensure_daemon()
        write_request("toggle")
    end)
    if not ok then vlc.msg.err("pip: " .. tostring(err)) end
end
