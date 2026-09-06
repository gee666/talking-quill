param(
    [Parameter(Mandatory = $true)][ValidateSet('x64', 'arm64')][string]$Architecture,
    [Parameter(Mandatory = $true)][string]$RuntimeRoot,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Never use this observer on a user's interactive installation. It deliberately
# launches the unmodified production entry, without acceptance or Chromium flags.
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or
    $env:RUNNER_OS -cne 'Windows' -or $env:GITHUB_RUN_ID -notmatch '^[1-9][0-9]*$') {
    throw 'Production startup observation requires a fresh GitHub-hosted Windows runner'
}
$nativeArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($nativeArchitecture -cne $Architecture) { throw 'Production observer requires the native OS architecture' }
$root = [IO.Path]::GetFullPath($RuntimeRoot).TrimEnd('\')
$output = [IO.Path]::GetFullPath($OutputDirectory).TrimEnd('\')
$tmpRoot = [IO.Path]::GetFullPath((Join-Path (Get-Location) 'tmp')).TrimEnd('\') + '\'
if (-not $output.StartsWith($tmpRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Production observation output must remain under project tmp'
}
[IO.Directory]::CreateDirectory($output) | Out-Null
$profile = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'Talking Quill'
if (Test-Path -LiteralPath $profile) { throw 'Production observation requires an absent default Talking Quill profile' }
$application = Join-Path $root 'Talking Quill.exe'
$helper = Join-Path $root 'resources\helper\talking-quill-helper.exe'
$owner = Join-Path $root 'resources\helper\talking-quill-keyboard-owner.exe'
foreach ($file in @($application, $helper, $owner)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing packaged executable: $file" }
}
function Redact([string]$Text) {
    foreach ($path in @($profile, $env:USERPROFILE, $env:APPDATA, $env:LOCALAPPDATA)) {
        if ($path) { $Text = $Text.Replace($path.Replace('\', '\\'), '<path>').Replace($path, '<path>') }
    }
    $Text = $Text -replace '(?i)(Bearer\s+)[^\s"'']+', '$1<redacted>'
    $Text = $Text -replace '(?i)((?:token|secret|password|api[-_]?key|correlation)\s*["'']?\s*[:=]\s*["'']?)[^\s"'',}]+', '$1<redacted>'
    return ($Text -replace '\b[0-9a-fA-F]{64}\b', '<redacted>' -replace 'S-1-5-(?:\d+-)*\d+', '<sid>')
}
function Save-Json([string]$Name, $Value) {
    [IO.File]::WriteAllText((Join-Path $output $Name), (Redact (ConvertTo-Json -InputObject $Value -Depth 12)), [Text.UTF8Encoding]::new($false))
}
function Package-Processes {
    $prefix = $root + '\'
    @(Get-CimInstance Win32_Process -OperationTimeoutSec 10 | Where-Object {
        $path = [string]$_.ExecutablePath
        if ($path.StartsWith('\\?\')) { $path = $path.Substring(4) }
        $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
    } | ForEach-Object {
        $path = [string]$_.ExecutablePath
        if ($path.StartsWith('\\?\')) { $path = $path.Substring(4) }
        try { $created = ([Diagnostics.Process]::GetProcessById([int]$_.ProcessId)).StartTime.ToUniversalTime().ToString('o') }
        catch { return } # A process can exit between CIM enumeration and identity capture.
        [pscustomobject]@{
            pid = [int]$_.ProcessId; parentPid = [int]$_.ParentProcessId
            path = $path; created = $created
            renderer = ([string]$_.CommandLine -match '(?:^|\s)--type=renderer(?:\s|$)')
        }
    })
}
if (@(Package-Processes).Count -ne 0) { throw 'Production observation requires no existing package processes' }
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase, System.Drawing
Add-Type -Path (Join-Path $PSScriptRoot 'windows-production-startup-observer.cs') -ReferencedAssemblies UIAutomationClient, UIAutomationTypes, WindowsBase, System.Drawing

$clock = [Diagnostics.Stopwatch]::StartNew()
$capture = $null
$known = @{}
$lastWindows = @()
$lastProcesses = @()
$failure = $null
$observation = $null
$stable = 0
$previousIdentity = ''
$cleanup = [ordered]@{ closeRequested = $false; forcedTermination = $false; forcedPids = @(); remainingPackageProcesses = -1 }
function Track-Processes($Processes) {
    # Record descendants while their parents are still alive, including detached owner.
    do {
        $changed = $false
        foreach ($item in $Processes) {
            if (-not $known.ContainsKey($item.pid) -and $known.ContainsKey($item.parentPid)) {
                $known[$item.pid] = $item
                $changed = $true
            }
        }
    } while ($changed)
}
try {
    $capture = [StartupCapture]::new($application, $root)
    $mainPid = $capture.Process.Id
    $known[$mainPid] = [pscustomobject]@{
        pid = $mainPid; parentPid = $PID; path = $application
        created = $capture.Process.StartTime.ToUniversalTime().ToString('o'); renderer = $false
    }
    Save-Json 'launch-context.json' ([ordered]@{
        architecture = $Architecture; executable = $application; cwd = $root
        arguments = @(); profile = $profile; profileAbsentBeforeLaunch = $true
        mainPid = $mainPid; sessionId = $capture.Process.SessionId
        elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    })
    while ($clock.ElapsedMilliseconds -lt 90000) {
        if ($capture.Process.HasExited) { throw "Production application exited before UI readiness: $($capture.Process.ExitCode)" }
        $lastProcesses = @(Package-Processes)
        Track-Processes $lastProcesses
        $lastWindows = @([StartupObserver]::Windows($mainPid) | ForEach-Object {
            if ($clock.ElapsedMilliseconds -lt 90000) { [StartupObserver]::Accessibility($_) } else { $_ }
        })
        Save-Json 'windows.json' $lastWindows
        Save-Json 'processes.json' $lastProcesses
        foreach ($window in $lastWindows) {
            if ($window.ClassName -eq '#32770' -or $window.Title -match '(?i)startup failed|application error|javascript error' -or
                @($window.Elements | Where-Object { $_.Name -match '(?i)Talking Quill could not (start|load your settings)|Talking Quill startup failed' }).Count -gt 0) {
                try { [StartupObserver]::Screenshot($window.Handle, (Join-Path $output 'error-window.png')) } catch {}
                throw 'Production application displayed an error dialog or startup error content'
            }
        }
        $readyWindow = $null
        foreach ($window in $lastWindows) {
            $content = @($window.Elements | Where-Object { $_.Depth -gt 0 -and -not $_.Offscreen })
            $brand = @($content | Where-Object { $_.Name -ceq 'Talking Quill' -and $_.ControlType -eq 'ControlType.Text' }).Count -gt 0
            $welcome = @($content | Where-Object { $_.Name -ceq 'Welcome' }).Count -gt 0
            $continue = @($content | Where-Object { $_.Name -ceq 'Continue' -and $_.ControlType -eq 'ControlType.Button' }).Count -gt 0
            if ($window.Title -eq 'Talking Quill' -and $window.Width -ge 400 -and $window.Height -ge 300 -and $brand -and $welcome -and $continue) {
                $readyWindow = $window
                break
            }
        }
        $helpers = @($lastProcesses | Where-Object { $_.path -ieq $helper -and $known.ContainsKey($_.pid) })
        $owners = @($lastProcesses | Where-Object { $_.path -ieq $owner -and $known.ContainsKey($_.pid) })
        $renderers = @($lastProcesses | Where-Object { $_.path -ieq $application -and $_.renderer -and $known.ContainsKey($_.pid) })
        if ($null -ne $readyWindow -and $helpers.Count -eq 1 -and $owners.Count -eq 1 -and $renderers.Count -gt 0 -and
            $helpers[0].parentPid -eq $mainPid -and $owners[0].parentPid -eq $helpers[0].pid) {
            $identity = "$($readyWindow.Handle):$($helpers[0].pid):$($owners[0].pid)"
            if ($identity -ceq $previousIdentity) { $stable++ } else { $stable = 1 }
            $previousIdentity = $identity
            if ($stable -ge 2) {
                [StartupObserver]::Screenshot($readyWindow.Handle, (Join-Path $output 'startup-window.png'))
                $observation = [ordered]@{
                    mainPid = $mainPid; stableSamples = $stable
                    window = [ordered]@{
                        pid = $mainPid; visible = $true; title = $readyWindow.Title
                        width = $readyWindow.Width; height = $readyWindow.Height
                        accessibilitySource = 'Windows.UIAutomation'
                        markers = @('Talking Quill', 'Welcome', 'Continue')
                        contentElementCount = @($readyWindow.Elements | Where-Object { $_.Depth -gt 0 }).Count
                    }
                    helper = @{ pid = $helpers[0].pid; parentPid = $helpers[0].parentPid; relativePath = 'resources/helper/talking-quill-helper.exe' }
                    owner = @{ pid = $owners[0].pid; parentPid = $owners[0].parentPid; relativePath = 'resources/helper/talking-quill-keyboard-owner.exe' }
                    rendererPids = @($renderers | ForEach-Object { $_.pid })
                }
                break
            }
        } else { $stable = 0; $previousIdentity = '' }
        Start-Sleep -Milliseconds 1000
    }
    if ($null -eq $observation) { throw 'Production UI/helper/owner startup observation timed out after 90000ms' }
} catch {
    $failure = $_.Exception.Message
} finally {
    if ($null -ne $capture) {
        try {
            # WM_CLOSE is a normal UI close request. Closing to tray is not process exit.
            foreach ($window in @([StartupObserver]::Windows($capture.Process.Id))) {
                if ([StartupObserver]::Close($window.Handle)) { $cleanup.closeRequested = $true }
            }
            $closeDeadline = [DateTime]::UtcNow.AddSeconds(5)
            do {
                $remaining = @(Package-Processes)
                Track-Processes $remaining
                if ($remaining.Count -eq 0) { break }
                Start-Sleep -Milliseconds 250
            } while ([DateTime]::UtcNow -lt $closeDeadline)
            # Never kill by image name, and never kill an untracked process under the root.
            foreach ($item in @($remaining | Sort-Object { $_.pid -eq $capture.Process.Id })) {
                if (-not $known.ContainsKey($item.pid)) { continue }
                $expected = $known[$item.pid]
                if ($item.path -ine $expected.path -or $item.created -cne $expected.created) { continue }
                $process = Get-Process -Id $item.pid -ErrorAction SilentlyContinue
                if ($null -ne $process -and $process.StartTime.ToUniversalTime().ToString('o') -ceq $expected.created) {
                    $cleanup.forcedTermination = $true
                    $cleanup.forcedPids += $item.pid
                    $process.Kill()
                    $process.WaitForExit(3000) | Out-Null
                }
            }
            $cleanup.remainingPackageProcesses = @(Package-Processes).Count
            if ($cleanup.remainingPackageProcesses -ne 0) { throw 'Package processes remained after bounded observer cleanup' }
        } catch {
            $cleanup['error'] = $_.Exception.Message
            if ($null -eq $failure) { $failure = 'Production observation cleanup failed' }
        }
        $capture.Finish()
        [IO.File]::WriteAllText((Join-Path $output 'application.stdout.txt'), (Redact $capture.Output()))
        [IO.File]::WriteAllText((Join-Path $output 'application.stderr.txt'), (Redact $capture.Error()))
        Save-Json 'application-exit.json' (@{
            exited = $capture.Process.HasExited
            exitCode = $(if ($capture.Process.HasExited) { $capture.Process.ExitCode } else { $null })
            stdoutChars = $capture.OutputChars(); stderrChars = $capture.ErrorChars()
            stdoutTruncated = $capture.OutputChars() -gt [StartupCapture]::Limit
            stderrTruncated = $capture.ErrorChars() -gt [StartupCapture]::Limit
        })
    }
    $logs = @()
    foreach ($suffix in @('', '.1', '.2')) {
        $name = 'diagnostic.jsonl' + $suffix
        $stream = $null
        try {
            $stream = [IO.File]::Open((Join-Path $profile ('logs\' + $name)), 'Open', 'Read', 'ReadWrite')
            $buffer = New-Object byte[] 262144
            $count = $stream.Read($buffer, 0, $buffer.Length)
            [IO.File]::WriteAllText((Join-Path $output ($name + '.txt')), (Redact ([Text.Encoding]::UTF8.GetString($buffer, 0, $count))))
            $logs += @{ name = $name; bytes = $stream.Length; truncated = $stream.Length -gt $count }
        } catch { $logs += @{ name = $name; error = $_.Exception.GetType().Name } }
        finally { if ($null -ne $stream) { $stream.Dispose() } }
    }
    Save-Json 'app-logs.json' $logs
    Save-Json 'cleanup.json' $cleanup
}
$report = [ordered]@{
    schemaVersion = 1; kind = 'windows-production-startup-observation'
    result = $(if ($null -eq $failure) { 'passed' } else { 'failed' })
    architecture = $Architecture; mode = 'unpacked'
    launch = @{ arguments = @(); testHooks = $false; profile = 'fresh-hosted-default' }
    coverage = @{ startup = $(if ($null -ne $observation) { 'observed' } else { 'not-observed' }); ownerAuthentication = 'not-observed'; transactions = 'not-exercised'; gracefulLifecycle = 'not-asserted' }
    observation = $observation; cleanup = $cleanup; durationMs = $clock.ElapsedMilliseconds
    screenshot = $(if ($null -ne $observation) { 'startup-window.png' } else { $null })
    failure = $failure
}
Save-Json 'startup-report.json' $report
if ($null -ne $failure) { throw (Redact $failure) }
