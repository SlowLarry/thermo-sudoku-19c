using System.Diagnostics;
using System.Runtime.InteropServices.JavaScript;
using System.Text.Json;
using System.Text.Json.Serialization;
using SudokuSolver;
using SudokuSolverBenchmark;

namespace SudokuSolverWasm;

internal sealed class ThermoBenchRequest
{
    public BenchCase[] Cases { get; set; } = [];
    public int WarmupRounds { get; set; }
    public int Repeats { get; set; }
}

internal sealed class ThermoBenchCaseResult
{
    public string Name { get; set; } = "";
    public long Observed { get; set; }
    public double[] BuildMs { get; set; } = [];
    public double[] CountMs { get; set; } = [];
    public double[] TotalMs { get; set; } = [];
}

internal sealed class ThermoBenchResponse
{
    public ThermoBenchCaseResult[] Cases { get; set; } = [];
}

[JsonSerializable(typeof(ThermoBenchRequest))]
[JsonSerializable(typeof(ThermoBenchResponse))]
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase)]
internal partial class ThermoBenchJsonContext : JsonSerializerContext
{
}

/// <summary>
/// Timing-only export for the thermo comparison. Unlike the prototype's general benchmark core,
/// this deliberately includes fresh solver construction because every annealing proposal changes
/// the thermometer layout.
/// </summary>
public static partial class ThermoBenchInterop
{
    [JSExport]
    public static string RunBatch(string requestJson)
    {
        ThermoBenchRequest request = JsonSerializer.Deserialize(
            requestJson,
            ThermoBenchJsonContext.Default.ThermoBenchRequest)
            ?? throw new ArgumentException("Invalid benchmark request.", nameof(requestJson));
        if (request.Cases.Length == 0)
        {
            throw new ArgumentException("At least one benchmark case is required.", nameof(requestJson));
        }
        if (request.WarmupRounds < 0 || request.Repeats <= 0)
        {
            throw new ArgumentException("Warm-up rounds must be non-negative and repeats positive.", nameof(requestJson));
        }

        var results = request.Cases.Select(c => new ThermoBenchCaseResult
        {
            Name = c.Name,
            BuildMs = new double[request.Repeats],
            CountMs = new double[request.Repeats],
            TotalMs = new double[request.Repeats],
        }).ToArray();

        for (int round = 0; round < request.WarmupRounds; round++)
        {
            for (int offset = 0; offset < request.Cases.Length; offset++)
            {
                int index = (offset + round) % request.Cases.Length;
                BenchCase benchCase = request.Cases[index];
                Solver solver = BenchCore.Build(benchCase);
                long observed = BenchCore.RunOp(solver, benchCase, forceMultiThread: false);
                Validate(benchCase, observed);
            }
        }

        for (int round = 0; round < request.Repeats; round++)
        {
            for (int offset = 0; offset < request.Cases.Length; offset++)
            {
                int index = (offset + round) % request.Cases.Length;
                BenchCase benchCase = request.Cases[index];

                long started = Stopwatch.GetTimestamp();
                Solver solver = BenchCore.Build(benchCase);
                long built = Stopwatch.GetTimestamp();
                long observed = BenchCore.RunOp(solver, benchCase, forceMultiThread: false);
                long finished = Stopwatch.GetTimestamp();
                Validate(benchCase, observed);

                ThermoBenchCaseResult result = results[index];
                result.Observed = observed;
                result.BuildMs[round] = ElapsedMilliseconds(started, built);
                result.CountMs[round] = ElapsedMilliseconds(built, finished);
                result.TotalMs[round] = ElapsedMilliseconds(started, finished);
            }
        }

        return JsonSerializer.Serialize(
            new ThermoBenchResponse { Cases = results },
            ThermoBenchJsonContext.Default.ThermoBenchResponse);
    }

    private static double ElapsedMilliseconds(long start, long finish)
        => (finish - start) * 1000.0 / Stopwatch.Frequency;

    private static void Validate(BenchCase benchCase, long observed)
    {
        if (benchCase.Expected is long expected && observed != expected)
        {
            throw new InvalidOperationException(
                $"{benchCase.Name}: observed {observed}, expected {expected}.");
        }
    }
}
