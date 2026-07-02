-- Throwaway VLC interface script: loads pip.lua inside VLC's real Lua sandbox,
-- calls descriptor() + trigger(), logs results, then quits VLC.
local ok, err = pcall(function()
    local path = os.getenv("APPDATA") .. "\\vlc\\lua\\extensions\\pip.lua"
    dofile(path)
    local d = descriptor()
    vlc.msg.info("pip-test: descriptor ok title=" .. tostring(d.title)
        .. " capability=" .. tostring(d.capabilities and d.capabilities[1]))
    trigger()
    vlc.msg.info("pip-test: trigger ok")
    -- trigger() wrote a real "toggle" request; eat it so a running daemon does not
    -- surprise-PiP whatever other VLC window the user has open after we quit
    os.remove((os.getenv("TEMP") or os.getenv("TMP") or ".") .. "\\vlc-pip-request.txt")
end)
if not ok then vlc.msg.err("pip-test: FAILED " .. tostring(err)) end
vlc.misc.quit()
