' Launches herdr-agent-quota.exe with truly no console window, for Herdr's
' Windows startup/event manifest commands (the ones fired every few seconds
' by pane.agent_detected/agent_status_changed/focused).
'
' `powershell -WindowStyle Hidden` still briefly flashes a console: Windows
' allocates PowerShell's conhost window as part of spawning it, and
' PowerShell only hides that window a moment later, after it already
' appeared. wscript.exe is a GUI-subsystem host -- it never allocates a
' console window in the first place -- and Shell.Run's window-style argument
' is applied before the target process is even created, so nothing is ever
' shown to flash.
'
' Argument 0 is the herdr-agent-quota.exe subcommand line to run (e.g.
' "event", or "startup --provider all"), passed as one already-joined string
' by the manifest command that invokes this script.
Set shell = CreateObject("WScript.Shell")
root = shell.ExpandEnvironmentStrings("%HERDR_PLUGIN_ROOT%")
If Left(root, 4) = "\\?\" Then root = Mid(root, 5)
exePath = root & "\target\release\herdr-agent-quota.exe"
cmd = """" & exePath & """ " & WScript.Arguments(0)
WScript.Quit shell.Run(cmd, 0, True)
