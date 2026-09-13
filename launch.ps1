param(
    [string]$AlphaSource = (Join-Path (Split-Path $PSScriptRoot -Parent) 'alpha-osk-uia-870f21a'),
    [string]$Python = "$env:USERPROFILE\repos\alpha-osk\venv\Scripts\python.exe",
    [switch]$LiveInput
)
$ErrorActionPreference = 'Stop'
$labRoot = $PSScriptRoot
$labBinary = Join-Path $labRoot 'target\release\alpha-osk-scan-lab.exe'
foreach ($required in @($Python, $labBinary, (Join-Path $AlphaSource 'qml\Main.qml'))) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required file missing: $required. See README.md." }
}
$fixtureWork = Join-Path $labRoot 'work\fixture'
New-Item -ItemType Directory -Force -Path $fixtureWork | Out-Null
$stateFile = Join-Path $fixtureWork 'fixture-state.json'
if (Test-Path -LiteralPath $stateFile) {
    $previousFixture = Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
    if (Get-Process -Id $previousFixture.pid -ErrorAction SilentlyContinue) {
        throw 'A fixture is already running. Close its keyboard window before launching another.'
    }
}
$fixtureScript = Join-Path $labRoot 'scripts\alpha_fixture.py'
# Start-Process joins ArgumentList; quote every path to preserve spaces.
$fixtureArgs = @(('"' + $fixtureScript + '"'), '--alpha', ('"' + $AlphaSource + '"'), '--work', ('"' + $fixtureWork + '"'))
if ($LiveInput) { $fixtureArgs += '--live-input' }
Start-Process -FilePath $Python -ArgumentList $fixtureArgs -WorkingDirectory $labRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $labRoot 'work\fixture.stdout.log') -RedirectStandardError (Join-Path $labRoot 'work\fixture.stderr.log') | Out-Null
Start-Process -FilePath $labBinary -WorkingDirectory $labRoot -WindowStyle Hidden | Out-Null
Write-Host "Started Scan Lab and Alpha fixture. Live input: $($LiveInput.IsPresent). Close both windows when finished."
