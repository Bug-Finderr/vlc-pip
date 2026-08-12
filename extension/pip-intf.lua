-- Lua interface companion (installs as lua\intf\pip.lua; enabled via vlcrc
-- extraintf=luaintf + lua-intf=pip). Publishes the playing video's size for the
-- daemon: one line "epoch v=WxH" ("epoch -" without a sized video), rewritten every
-- poll so consumers gate on freshness like the daemon heartbeat. The daemon uses it
-- to adapt hotkey/CLI enters and to retarget a live PiP when the playlist moves to
-- a differently shaped video.

-- TMP before TEMP: the daemon resolves %TEMP% via GetTempPath, which checks TMP first,
-- and a machine where the two differ must not split publisher and consumer.
local function temp_dir()
    return (os.getenv("TMP") or os.getenv("TEMP") or ".")
end

-- Same probe as pip.lua, kept standalone (VLC gives intf and extension scripts no
-- shared-module path). item:info() keys are localized: match the English "Video
-- resolution" only - shape-scanning values would let the padded "Buffer dimensions"
-- or a WxH-shaped stream description hijack the box. Transposed orientations
-- ("Left ..."/"Right ...") display with the axes swapped.
local function video_dims()
    local ok, item = pcall(function() return vlc.input.item() end)
    if not ok or not item then return nil end
    local ok2, info = pcall(function() return item:info() end)
    if not ok2 or type(info) ~= "table" then return nil end
    for _, cat in pairs(info) do
        if type(cat) == "table" and cat["Video resolution"] then
            local w, h = tostring(cat["Video resolution"]):match("^(%d+)x(%d+)$")
            if w then
                if tostring(cat["Orientation"] or ""):match("^[LR]") then w, h = h, w end
                return w .. "x" .. h
            end
        end
    end
    return nil
end

local path = temp_dir() .. "\\vlc-pip-media.txt"
local function publish_forever()
    while true do
        local dims = video_dims()
        local f = io.open(path, "w")
        if f then
            f:write(os.time() .. " " .. (dims and ("v=" .. dims) or "-"))
            f:close()
        end
        vlc.misc.mwait(vlc.misc.mdate() + 300000) -- 300ms; mdate/mwait are microseconds
    end
end
-- VLC 3's lua misc has no should_die: shutdown arrives as mwait's "Interrupted."
-- error, so the loop unwinds through pcall and the publication is removed.
pcall(publish_forever)
os.remove(path)
