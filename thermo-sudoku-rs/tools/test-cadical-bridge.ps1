param(
    [string] $Bridge = "target/release/cadical-incremental-bridge.exe"
)

$ErrorActionPreference = "Stop"
$bridgePath = (Resolve-Path -LiteralPath $Bridge).Path
$testDirectory = Join-Path ([System.IO.Path]::GetTempPath()) (
    "thermo-cadical-bridge-test-{0}-{1}" -f $PID, [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
)
[System.IO.Directory]::CreateDirectory($testDirectory) | Out-Null
$satPath = Join-Path $testDirectory "sat.cnf"
$unknownPath = Join-Path $testDirectory "unknown.cnf"
$truncatedPath = Join-Path $testDirectory "truncated.cnf"
$extraPath = Join-Path $testDirectory "extra.cnf"
$unterminatedPath = Join-Path $testDirectory "unterminated.cnf"
$outOfRangePath = Join-Path $testDirectory "out-of-range.cnf"
$doubleSpaceHeaderPath = Join-Path $testDirectory "double-space-header.cnf"
$trailingSpaceHeaderPath = Join-Path $testDirectory "trailing-space-header.cnf"
$leadingBlankHeaderPath = Join-Path $testDirectory "leading-blank-header.cnf"
[System.IO.File]::WriteAllText($satPath, "p cnf 2 2`n1 2 0`n-1 2 0`n")
[System.IO.File]::WriteAllText(
    $unknownPath,
    "p cnf 2 4`n1 2 0`n1 -2 0`n-1 2 0`n-1 -2 0`n"
)
[System.IO.File]::WriteAllText($truncatedPath, "p cnf 2 3`n1 2 0`n-1 2 0`n")
[System.IO.File]::WriteAllText($extraPath, "p cnf 2 1`n1 2 0`n-1 2 0`n")
[System.IO.File]::WriteAllText($unterminatedPath, "p cnf 2 1`n1 2`n")
[System.IO.File]::WriteAllText($outOfRangePath, "p cnf 2 1`n3 0`n")
[System.IO.File]::WriteAllText($doubleSpaceHeaderPath, "p cnf  2 1`n1 0`n")
[System.IO.File]::WriteAllText($trailingSpaceHeaderPath, "p cnf 2 1 `n1 0`n")
[System.IO.File]::WriteAllText($leadingBlankHeaderPath, "`np cnf 2 1`n1 0`n")
$processes = @()

function Assert-Equal([string] $Actual, [string] $Expected, [string] $Label) {
    if ($Actual -cne $Expected) {
        throw "$Label mismatch: expected '$Expected', got '$Actual'"
    }
}

function Start-TestBridge([string] $Cnf, [int] $Variables, [int] $Clauses) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $bridgePath
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @(
        "--cnf", $Cnf, "--variables", $Variables.ToString(), "--clauses", $Clauses.ToString()
    )) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "Could not start bridge"
    }
    $script:processes += $process
    return $process
}

try {
    $sat = Start-TestBridge $satPath 2 2
    $ready = $sat.StandardOutput.ReadLine()
    if (-not $ready.StartsWith("READY thermo-cadical-bridge-v1 variables=2 clauses=2 ")) {
        throw "Malformed READY response: '$ready'"
    }
    $sat.StandardInput.WriteLine("SOLVE -1")
    $sat.StandardInput.Flush()
    Assert-Equal $sat.StandardOutput.ReadLine() "RESULT SAT 2" "SAT status"
    $model = $sat.StandardOutput.ReadLine().Split(' ', [System.StringSplitOptions]::RemoveEmptyEntries)
    if ($model.Length -ne 4 -or $model[0] -cne "MODEL" -or $model[3] -cne "0") {
        throw "Malformed complete model"
    }
    $sat.StandardInput.WriteLine("ADD 0 0")
    $sat.StandardInput.Flush()
    Assert-Equal $sat.StandardOutput.ReadLine() "ADDED 1 0 3" "empty-clause acknowledgement"
    $sat.StandardInput.WriteLine("SOLVE -1")
    $sat.StandardInput.Flush()
    Assert-Equal $sat.StandardOutput.ReadLine() "RESULT UNSAT" "UNSAT status"
    $sat.StandardInput.WriteLine("QUIT")
    $sat.StandardInput.Flush()
    Assert-Equal $sat.StandardOutput.ReadLine() "BYE 1" "shutdown"
    $sat.WaitForExit()
    if ($sat.ExitCode -ne 0) {
        throw "SAT/ADD/UNSAT bridge exited $($sat.ExitCode)"
    }

    $unknown = Start-TestBridge $unknownPath 2 4
    $null = $unknown.StandardOutput.ReadLine()
    $unknown.StandardInput.WriteLine("SOLVE 0")
    $unknown.StandardInput.Flush()
    Assert-Equal $unknown.StandardOutput.ReadLine() "RESULT UNKNOWN" "UNKNOWN status"
    $unknown.StandardInput.WriteLine("SOLVE -1")
    $unknown.StandardInput.Flush()
    Assert-Equal $unknown.StandardOutput.ReadLine() "RESULT UNSAT" "post-UNKNOWN unlimited status"
    $unknown.StandardInput.WriteLine("QUIT")
    $unknown.StandardInput.Flush()
    Assert-Equal $unknown.StandardOutput.ReadLine() "BYE 0" "UNKNOWN shutdown"
    $unknown.WaitForExit()
    if ($unknown.ExitCode -ne 0) {
        throw "UNKNOWN bridge exited $($unknown.ExitCode)"
    }

    $wrongHeader = Start-TestBridge $satPath 2 3
    $wrongHeader.WaitForExit()
    if ($wrongHeader.ExitCode -eq 0 -or
        -not $wrongHeader.StandardError.ReadToEnd().Contains("DIMACS header disagrees")) {
        throw "Header/count mismatch was not rejected"
    }

    $truncated = Start-TestBridge $truncatedPath 2 3
    $truncated.WaitForExit()
    $truncatedError = $truncated.StandardError.ReadToEnd()
    if ($truncated.ExitCode -eq 0 -or
        -not $truncatedError.Contains("cannot parse CNF:") -or
        -not $truncatedError.Contains("clause missing")) {
        throw "Truncated DIMACS clause body was not rejected: '$truncatedError'"
    }

    $extra = Start-TestBridge $extraPath 2 1
    $extra.WaitForExit()
    $extraError = $extra.StandardError.ReadToEnd()
    if ($extra.ExitCode -eq 0 -or
        -not $extraError.Contains("cannot parse CNF:") -or
        -not $extraError.Contains("too many clauses")) {
        throw "Extra DIMACS clause body was not rejected: '$extraError'"
    }

    $unterminated = Start-TestBridge $unterminatedPath 2 1
    $unterminated.WaitForExit()
    $unterminatedError = $unterminated.StandardError.ReadToEnd()
    if ($unterminated.ExitCode -eq 0 -or
        -not $unterminatedError.Contains("cannot parse CNF:") -or
        -not $unterminatedError.Contains("last clause without terminating '0'")) {
        throw "Unterminated DIMACS clause was not rejected: '$unterminatedError'"
    }

    $outOfRange = Start-TestBridge $outOfRangePath 2 1
    $outOfRange.WaitForExit()
    $outOfRangeError = $outOfRange.StandardError.ReadToEnd()
    if ($outOfRange.ExitCode -eq 0 -or
        -not $outOfRangeError.Contains("cannot parse CNF:") -or
        -not $outOfRangeError.Contains("literal 3 exceeds maximum variable 2")) {
        throw "Out-of-range DIMACS literal was not rejected: '$outOfRangeError'"
    }

    $doubleSpaceHeader = Start-TestBridge $doubleSpaceHeaderPath 2 1
    $doubleSpaceHeader.WaitForExit()
    $doubleSpaceHeaderError = $doubleSpaceHeader.StandardError.ReadToEnd()
    if ($doubleSpaceHeader.ExitCode -eq 0 -or
        -not $doubleSpaceHeaderError.Contains("cannot parse CNF:") -or
        -not $doubleSpaceHeaderError.Contains("expected digit after 'p cnf '")) {
        throw "Double-space DIMACS header was not rejected: '$doubleSpaceHeaderError'"
    }

    $trailingSpaceHeader = Start-TestBridge $trailingSpaceHeaderPath 2 1
    $trailingSpaceHeader.WaitForExit()
    $trailingSpaceHeaderError = $trailingSpaceHeader.StandardError.ReadToEnd()
    if ($trailingSpaceHeader.ExitCode -eq 0 -or
        -not $trailingSpaceHeaderError.Contains("cannot parse CNF:") -or
        -not $trailingSpaceHeaderError.Contains("expected new-line after 'p cnf 2 1'")) {
        throw "Trailing-space DIMACS header was not rejected: '$trailingSpaceHeaderError'"
    }

    $leadingBlankHeader = Start-TestBridge $leadingBlankHeaderPath 2 1
    $leadingBlankHeader.WaitForExit()
    $leadingBlankHeaderError = $leadingBlankHeader.StandardError.ReadToEnd()
    if ($leadingBlankHeader.ExitCode -eq 0 -or
        -not $leadingBlankHeaderError.Contains("cannot parse CNF:") -or
        -not $leadingBlankHeaderError.Contains("expected 'c' or 'p'")) {
        throw "Leading-blank DIMACS header was not rejected: '$leadingBlankHeaderError'"
    }

    $malformed = Start-TestBridge $satPath 2 2
    $null = $malformed.StandardOutput.ReadLine()
    $malformed.StandardInput.WriteLine("ADD 1 1")
    $malformed.StandardInput.Flush()
    Assert-Equal $malformed.StandardOutput.ReadLine() "ERROR invalid_ADD_size" "malformed ADD"
    $malformed.WaitForExit()
    if ($malformed.ExitCode -eq 0) {
        throw "Malformed ADD did not fail the bridge"
    }

    $embeddedZero = Start-TestBridge $satPath 2 2
    $null = $embeddedZero.StandardOutput.ReadLine()
    $embeddedZero.StandardInput.WriteLine("ADD 2 1 0 0")
    $embeddedZero.StandardInput.Flush()
    Assert-Equal $embeddedZero.StandardOutput.ReadLine() "ERROR invalid_ADD_literal" "embedded ADD zero"
    $embeddedZero.WaitForExit()
    if ($embeddedZero.ExitCode -eq 0) {
        throw "Embedded ADD zero did not fail the bridge"
    }

    Write-Output "cadical bridge protocol smoke tests passed"
}
finally {
    foreach ($process in $processes) {
        if (-not $process.HasExited) {
            $process.StandardInput.Close()
            if (-not $process.WaitForExit(1000)) {
                $process.Kill($true)
                $process.WaitForExit()
            }
        }
        $process.Dispose()
    }
    foreach ($file in @(
        $satPath, $unknownPath, $truncatedPath, $extraPath, $unterminatedPath,
        $outOfRangePath, $doubleSpaceHeaderPath, $trailingSpaceHeaderPath,
        $leadingBlankHeaderPath
    )) {
        if ([System.IO.File]::Exists($file)) {
            [System.IO.File]::Delete($file)
        }
    }
    if ([System.IO.Directory]::Exists($testDirectory)) {
        [System.IO.Directory]::Delete($testDirectory)
    }
}
