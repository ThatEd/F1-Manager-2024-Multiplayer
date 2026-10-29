#requires -Version 5.1
<#
.SYNOPSIS
    One real game (host) + one headless client: the cheapest end-to-end co-op test.

.DESCRIPTION
    Starts tools/coop_server.py, waits for the real in-game bridge to register (it polls, so it
    appears on its own if the game is running with the coop pak), adds a headless client, and then
    runs a small scripted sequence while printing what the server saw for each step.

    Read-only with respect to the game install. It never loads a save, never quits the game, and
    never sends anything that is refused by the page rule unless the step explicitly asks for it.

.PARAMETER Steps
    Comma-separated steps. Each is one of:
      status                 print /status and continue
      wait:<seconds>         sleep
      expect-frontend        require the real client's page to be Frontend
      cmd:<text>             queue a command (spaces allowed, e.g. "cmd:Advance")
      headless               start the headless client for the rest of the run
      stop-headless          stop it

.EXAMPLE
    .\run_two_client_test.ps1 -Steps "status,headless,cmd:Advance,wait:3,status,stop-headless"
#>
[CmdletBinding()]
param(
    [string] $Steps = 'status,headless,cmd:Advance,wait:3,status,stop-headless',
    [int]    $Port  = 45999,
    # coop_server.py keys clients on source IP, so a harness on the same machine as the real
    # bridge must come from a different loopback address or the two collide into one entry.
    [string] $LocalAddress = '127.0.0.2',
    [switch] $KeepServerRunning
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$server = Join-Path $root 'tools\coop_server.py'
$client = Join-Path $PSScriptRoot 'coop_client.js'
$base = "http://127.0.0.1:$Port"

function Get-Status {
    try { return (Invoke-WebRequest "$base/status" -UseBasicParsing -TimeoutSec 5).Content | ConvertFrom-Json }
    catch { return $null }
}

function Wait-Server([int] $seconds = 15) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (Get-Status) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Wait-Client([int] $seconds, [scriptblock] $match) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        $s = Get-Status
        if ($s -and $s.clients) {
            foreach ($cid in $s.clients.PSObject.Properties.Name) {
                if (& $match $s.clients.$cid) { return $cid }
            }
        }
        Start-Sleep -Milliseconds 400
    }
    return $null
}

# Observed live on 2026-10-28: the real in-game bridge reports pkg=coop-bridge. Our harness reports
# pkg=headless. Anything else is unidentified and treated as "real" for reporting purposes only.
function Is-Headless($c) { return $c.pkg -eq 'headless' }
function Is-Bridge($c)   { return $c.pkg -eq 'coop-bridge' }

function Test-ServerRefusal([string] $text, [int] $seconds = 8) {
    # Direct check of the server-side guard, independent of which clients are registered:
    # temporarily pretend our own client is in a career and see whether the job is skipped.
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        $s = Get-Status
        if ($s -and $s.history) {
            $hit = $s.history | Where-Object {
                $_.kind -eq 'skip' -and $_.cmd -eq $text -and $_.why -match 'front-end'
            } | Select-Object -First 1
            if ($hit) { return $hit }
        }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

if (-not (Test-Path $server)) { throw "missing $server" }
if (-not (Test-Path $client)) { throw "missing $client" }

$game = Get-Process -Name F1Manager24 -ErrorAction SilentlyContinue
if (-not $game) { Write-Warning 'F1Manager24.exe is not running; the real-client steps will time out.' }

$serverProc = $null
$clientProc = $null
$results = New-Object System.Collections.Generic.List[object]

function Add-Result($step, $verdict, $detail) {
    $results.Add([pscustomobject]@{ Step = $step; Verdict = $verdict; Detail = $detail })
    $colour = switch ($verdict) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host ("  {0,-6} {1,-22} {2}" -f $verdict, $step, $detail) -ForegroundColor $colour
}

try {
    if (-not (Get-Status)) {
        Write-Host "starting coop_server.py on port $Port ..."
        $serverProc = Start-Process -FilePath 'python' -ArgumentList $server -PassThru -WindowStyle Minimized
        if (-not (Wait-Server 20)) { throw 'server did not come up' }
        Write-Host 'server up.'
    } else {
        Write-Host 'server already running.'
    }

    $realId = Wait-Client 20 { param($c) -not (Is-Headless $c) }
    if ($realId) { Write-Host "real (in-game) client registered: $realId" -ForegroundColor Green }
    else { Write-Warning 'no real (in-game) client registered; is the game running with the coop pak?' }

    foreach ($raw in ($Steps -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
        switch -Regex ($raw) {

            '^status$' {
                $s = Get-Status
                $n = @($s.clients.PSObject.Properties).Count
                Add-Result 'status' 'INFO' "$n client(s), queue depth $(@($s.queue).Count)"
            }

            '^wait:(\d+(?:\.\d+)?)$' {
                $secs = [double]$Matches[1]
                Start-Sleep -Seconds $secs
                Add-Result 'wait' 'INFO' "$secs s"
            }

            '^expect-frontend$' {
                # Scoped to the real bridge: the black-screen rule is about where the *game* is,
                # and a headless client's page is a label we chose, not evidence about the game.
                $s = Get-Status
                $bridge = $null
                foreach ($cid in $s.clients.PSObject.Properties.Name) {
                    if (Is-Bridge $s.clients.$cid) { $bridge = $s.clients.$cid; break }
                }
                if (-not $bridge) {
                    Add-Result 'expect-frontend' 'FAIL' 'no in-game bridge registered'
                } else {
                    Add-Result 'expect-frontend' $(if ($bridge.page -eq 'Frontend') { 'PASS' } else { 'FAIL' }) `
                        ("bridge page=" + $bridge.page + " stage=" + $bridge.stage)
                }
            }

            '^report-bridge$' {
                $s = Get-Status
                $any = $false
                foreach ($cid in $s.clients.PSObject.Properties.Name) {
                    $c = $s.clients.$cid
                    if (Is-Bridge $c) {
                        Add-Result 'report-bridge' 'INFO' ("BRIDGE {0}: page={1} stage={2} sent={3}" -f $cid,$c.page,$c.stage,$c.sent)
                        $any = $true
                    } else {
                        Add-Result 'report-bridge' 'INFO' ("{0}: pkg={1} page={2}" -f $cid,$c.pkg,$c.page)
                    }
                }
                if (-not $any) { Add-Result 'report-bridge' 'INFO' 'no in-game bridge registered' }
            }

            '^cmd:(.+)$' {
                $text = $Matches[1]
                $enc = [uri]::EscapeDataString($text)

                # Snapshot the bridge's own counter before queueing: the bridge increments `sent`
                # when it accepts a command, so a delta proves receipt even if the folded ack has
                # not landed on a poll yet (the bridge folds acks into the NEXT /hello).
                $before = Get-Status
                $sentBefore = -1
                if ($before) {
                    foreach ($cid in $before.clients.PSObject.Properties.Name) {
                        if (Is-Bridge $before.clients.$cid) { $sentBefore = [int]$before.clients.$cid.sent }
                    }
                }

                $null = (Invoke-WebRequest "$base/cmd?text=$enc&ttl=60" -UseBasicParsing -TimeoutSec 5).Content
                Start-Sleep -Milliseconds 1500

                $s = Get-Status
                $delivered = @($s.history | Where-Object { $_.kind -eq 'deliver' -and $_.cmd -eq $text }).Count
                $skipped   = @($s.history | Where-Object { $_.kind -eq 'skip'    -and $_.cmd -eq $text }).Count
                $ackLine   = @($s.history | Where-Object { $_.kind -eq 'ack' -and $_.text -and $_.text -match [regex]::Escape($text) })

                $sentAfter = -1
                foreach ($cid in $s.clients.PSObject.Properties.Name) {
                    if (Is-Bridge $s.clients.$cid) { $sentAfter = [int]$s.clients.$cid.sent }
                }

                $ackTxt = if ($ackLine.Count) { $ackLine[-1].text } else { 'not yet (folded into next poll)' }
                $sentTxt = if ($sentBefore -ge 0 -and $sentAfter -ge 0) { "bridge sent $sentBefore->$sentAfter" } else { 'no bridge' }
                $detail = "delivered=$delivered skipped=$skipped | $sentTxt | ack=$ackTxt"

                $verdict = 'FAIL'
                if ($sentBefore -lt 0 -and $sentAfter -lt 0) {
                    # No in-game bridge in the registry at either end of this step. The command was
                    # queued and (possibly) taken by the headless client; we cannot claim more.
                    $verdict = if ($delivered -gt 0) { 'DELIVERED' } else { 'QUEUED' }
                } elseif ($delivered -gt 0) {
                    # delivered to SOMEONE. If the bridge accepted it, that is the strongest signal.
                    if ($sentBefore -ge 0 -and $sentAfter -gt $sentBefore) { $verdict = 'PASS' }
                    elseif ($ackLine.Count) { $verdict = 'PASS' }
                    else { $verdict = 'DELIVERED' }
                } elseif ($skipped -gt 0) { $verdict = 'REFUSED' }

                Add-Result "cmd:$text" $verdict $detail
            }

            '^headless$' {
                if ($clientProc) { Add-Result 'headless' 'INFO' 'already running'; break }
                $clientProc = Start-Process -FilePath 'node' -ArgumentList @(
                    $client,
                    '--mode=none',
                    '--page=Frontend',
                    ('--port=' + $Port),
                    ('--local-address=' + $LocalAddress),
                    '--quiet'
                ) -PassThru -WindowStyle Hidden
                $deadline = (Get-Date).AddSeconds(12)
                $found = $false
                while ((Get-Date) -lt $deadline -and -not $found) {
                    $s = Get-Status
                    foreach ($cid in $s.clients.PSObject.Properties.Name) {
                        if ($s.clients.$cid.pkg -eq 'headless') { $found = $true }
                    }
                    if (-not $found) { Start-Sleep -Milliseconds 300 }
                }
                Add-Result 'headless' $(if ($found) { 'PASS' } else { 'FAIL' }) 'headless client registered'
            }

            '^career-headless$' {
                # Restart our headless client as if it were inside a career, so the server-side
                # guard can be exercised without touching the real player's session.
                if ($clientProc) { try { Stop-Process -Id $clientProc.Id -Force -ErrorAction Stop } catch { }; $clientProc = $null; Start-Sleep -Milliseconds 400 }
                $clientProc = Start-Process -FilePath 'node' -ArgumentList @(
                    $client,
                    '--mode=none',
                    '--page=RaceWeekend',
                    '--stage=ingame',
                    ('--port=' + $Port),
                    ('--local-address=' + $LocalAddress),
                    '--quiet'
                ) -PassThru -WindowStyle Hidden
                $deadline = (Get-Date).AddSeconds(15)
                $found = $false
                while ((Get-Date) -lt $deadline -and -not $found) {
                    $s = Get-Status
                    if ($s -and $s.clients) {
                        foreach ($cid in $s.clients.PSObject.Properties.Name) {
                            if ($s.clients.$cid.pkg -eq 'headless' -and $s.clients.$cid.page -eq 'RaceWeekend') { $found = $true }
                        }
                    }
                    if (-not $found) { Start-Sleep -Milliseconds 300 }
                }
                Add-Result 'career-headless' $(if ($found) { 'PASS' } else { 'FAIL' }) 'headless client now reports page=RaceWeekend'
            }

            '^frontend-headless$' {
                if ($clientProc) { try { Stop-Process -Id $clientProc.Id -Force -ErrorAction Stop } catch { }; $clientProc = $null; Start-Sleep -Milliseconds 400 }
                $clientProc = Start-Process -FilePath 'node' -ArgumentList @(
                    $client,
                    '--mode=none',
                    '--page=Frontend',
                    ('--port=' + $Port),
                    ('--local-address=' + $LocalAddress),
                    '--quiet'
                ) -PassThru -WindowStyle Hidden
                $deadline = (Get-Date).AddSeconds(15)
                $found = $false
                while ((Get-Date) -lt $deadline -and -not $found) {
                    $s = Get-Status
                    if ($s -and $s.clients) {
                        foreach ($cid in $s.clients.PSObject.Properties.Name) {
                            if ($s.clients.$cid.pkg -eq 'headless' -and $s.clients.$cid.page -eq 'Frontend') { $found = $true }
                        }
                    }
                    if (-not $found) { Start-Sleep -Milliseconds 300 }
                }
                Add-Result 'frontend-headless' $(if ($found) { 'PASS' } else { 'FAIL' }) 'headless client back at Frontend'
            }

            '^expect-refused:(.+)$' {
                $text = $Matches[1]
                $enc = [uri]::EscapeDataString($text)
                $null = (Invoke-WebRequest "$base/cmd?text=$enc&ttl=30" -UseBasicParsing -TimeoutSec 5).Content
                $hit = Test-ServerRefusal $text 10
                $why = if ($hit) { $hit.why } else { 'no skip recorded' }
                Add-Result "expect-refused:$text" $(if ($hit) { 'PASS' } else { 'FAIL' }) $why
            }

            '^stop-headless$' {
                if ($clientProc) {
                    try { Stop-Process -Id $clientProc.Id -Force -ErrorAction Stop } catch { }
                    $clientProc = $null
                    Add-Result 'stop-headless' 'INFO' 'stopped'
                }
            }

            default { Add-Result $raw 'INFO' 'not a recognised step' }
        }
    }
}
finally {
    if ($clientProc) { try { Stop-Process -Id $clientProc.Id -Force -ErrorAction Stop } catch { } }
    if ($serverProc -and -not $KeepServerRunning) {
        try { Invoke-WebRequest "$base/quit" -UseBasicParsing -TimeoutSec 3 | Out-Null } catch { }
        Start-Sleep -Milliseconds 400
        try { Stop-Process -Id $serverProc.Id -Force -ErrorAction Stop } catch { }
    }
    Write-Host ''
    Write-Host '=== results ==='
    $results | Format-Table -AutoSize | Out-String | Write-Host
    $failed = @($results | Where-Object { $_.Verdict -eq 'FAIL' }).Count
    Write-Host ("{0} step(s), {1} failed" -f $results.Count, $failed)
    if ($failed -gt 0) { exit 1 }
}
