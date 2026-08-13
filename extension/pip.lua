-- capabilities={"trigger"}: VLC calls trigger() on every menu click (no checkmark state).

function descriptor()
    return {
        title = "PiP Mode",
        version = "2.1.4",
        author = "Sudharsan",
        shortdesc = "PiP Mode",
        description = "Toggle VLC into a borderless always-on-top corner window",
        capabilities = { "trigger" },
    }
end

-- Keep env lookups lazy: VLC probes descriptor() by executing the chunk top level,
-- where an error makes the extension disappear from the View menu. TMP before TEMP:
-- the daemon resolves %TEMP% via GetTempPath, which checks TMP first, and a machine
-- where the two differ must not split the request channel.
local function temp_dir()
    return (os.getenv("TMP") or os.getenv("TEMP") or ".")
end

local function appdata_dir()
    return (os.getenv("APPDATA") or ".")
end

local function daemon_alive()
    local f = io.open(temp_dir() .. "\\vlc-pip-daemon.alive", "r")
    if not f then return false end
    local ts = f:read("*n")
    f:close()
    -- A force-killed daemon leaves its heartbeat behind. A nil timestamp means this read
    -- raced truncate-then-write, so treat the daemon as alive and avoid a respawn flash.
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

-- Playing video's visible size as "WxH", or nil. item:info() keys are localized: match
-- the English "Video resolution" only (a localized VLC just skips adaptation) - shape-
-- scanning values instead would let the padded "Buffer dimensions" or a WxH-shaped
-- stream description hijack the box. VLC exposes no SAR-corrected size, so anamorphic
-- media adapts to its raster shape.
local function video_dims()
    local ok, item = pcall(function() return vlc.input.item() end)
    if not ok or not item then return nil end
    local ok2, info = pcall(function() return item:info() end)
    if not ok2 or type(info) ~= "table" then return nil end
    for _, cat in pairs(info) do
        if type(cat) == "table" and cat["Video resolution"] then
            local w, h = tostring(cat["Video resolution"]):match("^(%d+)x(%d+)$")
            if w then
                -- resolution is the pre-rotation raster; transposed orientations
                -- ("Left ..."/"Right ...") display with the axes swapped
                if tostring(cat["Orientation"] or ""):match("^[LR]") then w, h = h, w end
                return w .. "x" .. h
            end
        end
    end
    return nil
end

function trigger()
    local ok, err = pcall(function()
        ensure_daemon()
        local dims = video_dims()
        write_request(dims and ("toggle v=" .. dims) or "toggle")
    end)
    if not ok then vlc.msg.err("pip: " .. tostring(err)) end
end
