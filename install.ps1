# Install bddkit from a GitHub Release. Windows only - see install.sh for Linux/macOS.
#
#   irm https://raw.githubusercontent.com/bddkit/bddkit/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "bddkit/bddkit"
$Target = "x86_64-pc-windows-msvc" # the only Windows target the release workflow builds
$BinDir = if ($env:BDDKIT_INSTALL_DIR) { $env:BDDKIT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "bddkit\bin" }

Write-Host "> Resolving latest release..."
# GitHub's /releases/latest page 302s to /releases/tag/<tag>; the tag is the
# last path segment. No GitHub API call, no rate limit, no token.
$response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -SkipHttpErrorCheck
$location = $response.Headers.Location
if (-not $location) {
    Write-Error "Could not resolve the latest release tag."
    exit 1
}
$Tag = ($location -split "/")[-1]

$Name = "bddkit-$Tag-$Target"
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"

Write-Host "> Installing bddkit $Tag ($Target) to $BinDir"

$Archive = Join-Path $env:TEMP "$Name.zip"
$Checksum = Join-Path $env:TEMP "$Name.zip.sha256"

Invoke-WebRequest -Uri "$BaseUrl/$Name.zip" -OutFile $Archive
Invoke-WebRequest -Uri "$BaseUrl/$Name.zip.sha256" -OutFile $Checksum

$expected = (Get-Content $Checksum).Split(" ")[0].Trim().ToLower()
$actual = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) {
    Remove-Item $Archive, $Checksum -ErrorAction SilentlyContinue
    Write-Error "Checksum mismatch for $Name.zip (expected $expected, got $actual)"
    exit 1
}

New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
$UnpackDir = Join-Path $env:TEMP "bddkit-unpack-$Tag"
Expand-Archive -Path $Archive -DestinationPath $UnpackDir -Force
Copy-Item (Join-Path $UnpackDir "$Name\bddkit.exe") (Join-Path $BinDir "bddkit.exe") -Force
Remove-Item $UnpackDir, $Archive, $Checksum -Recurse -ErrorAction SilentlyContinue

Write-Host "$([char]0x2713) bddkit $Tag installed to $BinDir\bddkit.exe" -ForegroundColor Green

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (";$userPath;" -notlike "*;$BinDir;*") {
    Write-Host ""
    Write-Warning "$BinDir is not in your `$env:PATH"
    Write-Host "> Adding it to your user PATH (restart your terminal to pick it up)"
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
}
