param(
    [Parameter(Mandatory = $true)]
    [string] $CadicalRoot,

    [string] $Output = "target/release/cadical-incremental-bridge.exe",

    [string] $Compiler = "g++"
)

$ErrorActionPreference = "Stop"
$resolvedRoot = (Resolve-Path -LiteralPath $CadicalRoot).Path
$source = Join-Path $PSScriptRoot "cadical-incremental-bridge.cpp"
$header = Join-Path $resolvedRoot "src/cadical.hpp"
$library = Join-Path $resolvedRoot "build/libcadical.a"

if (-not (Test-Path -LiteralPath $header -PathType Leaf)) {
    throw "Missing CaDiCaL header: $header"
}
if (-not (Test-Path -LiteralPath $library -PathType Leaf)) {
    throw "Missing CaDiCaL static library: $library"
}

$revision = (& git -C $resolvedRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[0-9a-fA-F]{40}$') {
    throw "Could not determine the exact CaDiCaL Git revision"
}
$librarySha256 = (Get-FileHash -LiteralPath $library -Algorithm SHA256).Hash.ToLowerInvariant()

$outputPath = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Output))
$outputDirectory = Split-Path -Parent $outputPath
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null

$arguments = @(
    "-std=c++17",
    "-Wall",
    "-Wextra",
    "-Werror",
    "-O3",
    "-DNDEBUG",
    "-DTHERMO_CADICAL_REVISION=$revision",
    "-DTHERMO_CADICAL_LIBRARY_SHA256=$librarySha256",
    "-I$resolvedRoot/src",
    "-I$resolvedRoot/build",
    $source,
    $library,
    "-o",
    $outputPath
)

& $Compiler @arguments
if ($LASTEXITCODE -ne 0) {
    throw "Bridge compilation failed with exit code $LASTEXITCODE"
}

$executableSha256 = (Get-FileHash -LiteralPath $outputPath -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Output "bridge=$outputPath"
Write-Output "bridge_sha256=$executableSha256"
Write-Output "cadical_revision=$revision"
Write-Output "cadical_library_sha256=$librarySha256"
