param(
    [int]$Port = 9222,
    [string]$Profile = 'C:\chrome-profiles\codex-browser',
    [string]$StartUrl = 'about:blank',
    [switch]$KeepExisting
)

$ErrorActionPreference = 'Stop'

$chromeCandidates = @(
    'C:\Program Files\Google\Chrome\Application\chrome.exe',
    'C:\Program Files (x86)\Google\Chrome\Application\chrome.exe'
)

$chrome = $chromeCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $chrome) {
    throw "Chrome not found. Checked: $($chromeCandidates -join ', ')"
}

if (-not (Test-Path -LiteralPath $Profile)) {
    New-Item -ItemType Directory -Path $Profile -Force | Out-Null
}

if (-not $KeepExisting) {
    Get-CimInstance Win32_Process -Filter "name='chrome.exe'" |
        Where-Object {
            $_.CommandLine -and (
                $_.CommandLine -match [regex]::Escape($Profile) -or
                $_.CommandLine -match "--remote-debugging-port=$Port"
            )
        } |
        ForEach-Object {
            Write-Host ("Stopping old CDP Chrome PID {0}" -f $_.ProcessId)
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        }
    Start-Sleep -Milliseconds 700
}

$arguments = @(
    "--remote-debugging-port=$Port",
    "--user-data-dir=$Profile",
    '--no-first-run',
    '--no-default-browser-check',
    $StartUrl
)

Write-Host "Launching Chrome CDP on port $Port"
Write-Host "Profile: $Profile"
Start-Process -FilePath $chrome -ArgumentList $arguments | Out-Null

$endpoint = "http://127.0.0.1:$Port/json/version"
for ($i = 1; $i -le 15; $i++) {
    Start-Sleep -Seconds 1
    try {
        $response = Invoke-WebRequest -Uri $endpoint -UseBasicParsing -TimeoutSec 3
        $version = $response.Content | ConvertFrom-Json
        Write-Host ("OK CDP responding: {0}" -f $version.Browser) -ForegroundColor Green
        Write-Host "Endpoint: $endpoint"
        Write-Host "Leave this Chrome window open, then inspect it from WSL."
        exit 0
    } catch {
        Write-Host ("Waiting for CDP... {0}/15" -f $i)
    }
}

throw "Chrome started, but CDP did not respond at $endpoint"
