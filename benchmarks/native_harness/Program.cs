using System.Diagnostics;
using System.Runtime;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;
using SudokuSolver;

namespace NativeThermoBench;

internal sealed class BenchRequest
{
    public BenchCase[] Cases { get; set; } = [];
    public int WarmupRounds { get; set; }
    public int Repeats { get; set; }
}

internal sealed class BenchCase
{
    public string Name { get; set; } = "";
    public string Category { get; set; } = "";
    public int Blank { get; set; }
    public string[] Constraints { get; set; } = [];
    public string Op { get; set; } = "";
    public long Expected { get; set; }
    public long MaxCount { get; set; }
    public bool MultiThread { get; set; }
}

internal sealed class CaseResult
{
    public string Name { get; set; } = "";
    public long Observed { get; set; }
    public double[] BuildMs { get; set; } = [];
    public double[] CountMs { get; set; } = [];
    public double[] TotalMs { get; set; } = [];
}

internal sealed class RuntimeDetails
{
    public string FrameworkDescription { get; set; } = "";
    public string RuntimeIdentifier { get; set; } = "";
    public string ProcessArchitecture { get; set; } = "";
    public string OsArchitecture { get; set; } = "";
    public string GcMode { get; set; } = "";
    public long StopwatchFrequency { get; set; }
}

internal sealed class BenchResponse
{
    public CaseResult[] Cases { get; set; } = [];
    public RuntimeDetails Runtime { get; set; } = new();
}

[JsonSerializable(typeof(BenchRequest))]
[JsonSerializable(typeof(BenchResponse))]
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase)]
internal partial class BenchJsonContext : JsonSerializerContext
{
}

internal static class Program
{
    private static int Main()
    {
        try
        {
            string requestJson = Console.In.ReadToEnd();
            BenchRequest request = JsonSerializer.Deserialize(
                requestJson,
                BenchJsonContext.Default.BenchRequest)
                ?? throw new ArgumentException("Invalid benchmark request.");
            ValidateRequest(request);

            var results = request.Cases.Select(c => new CaseResult
            {
                Name = c.Name,
                BuildMs = new double[request.Repeats],
                CountMs = new double[request.Repeats],
                TotalMs = new double[request.Repeats],
            }).ToArray();

            RunWarmup(request);
            RunMeasurements(request, results);

            var response = new BenchResponse
            {
                Cases = results,
                Runtime = new RuntimeDetails
                {
                    FrameworkDescription = RuntimeInformation.FrameworkDescription,
                    RuntimeIdentifier = RuntimeInformation.RuntimeIdentifier,
                    ProcessArchitecture = RuntimeInformation.ProcessArchitecture.ToString(),
                    OsArchitecture = RuntimeInformation.OSArchitecture.ToString(),
                    GcMode = GCSettings.IsServerGC ? "server" : "workstation",
                    StopwatchFrequency = Stopwatch.Frequency,
                },
            };
            Console.Out.Write(JsonSerializer.Serialize(response, BenchJsonContext.Default.BenchResponse));
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error);
            return 1;
        }
    }

    private static void RunWarmup(BenchRequest request)
    {
        for (int round = 0; round < request.WarmupRounds; round++)
        {
            for (int offset = 0; offset < request.Cases.Length; offset++)
            {
                int index = (offset + round) % request.Cases.Length;
                BenchCase benchCase = request.Cases[index];
                Solver solver = Build(benchCase);
                long observed = Count(solver, benchCase);
                ValidateObserved(benchCase, observed);
            }
        }
    }

    private static void RunMeasurements(BenchRequest request, CaseResult[] results)
    {
        for (int round = 0; round < request.Repeats; round++)
        {
            for (int offset = 0; offset < request.Cases.Length; offset++)
            {
                int index = (offset + round) % request.Cases.Length;
                BenchCase benchCase = request.Cases[index];

                long started = Stopwatch.GetTimestamp();
                Solver solver = Build(benchCase);
                long built = Stopwatch.GetTimestamp();
                long observed = Count(solver, benchCase);
                long finished = Stopwatch.GetTimestamp();
                ValidateObserved(benchCase, observed);

                CaseResult result = results[index];
                result.Observed = observed;
                result.BuildMs[round] = ElapsedMilliseconds(started, built);
                result.CountMs[round] = ElapsedMilliseconds(built, finished);
                result.TotalMs[round] = ElapsedMilliseconds(started, finished);
            }
        }
    }

    private static Solver Build(BenchCase benchCase)
        => SolverFactory.CreateBlank(benchCase.Blank, benchCase.Constraints);

    private static long Count(Solver solver, BenchCase benchCase)
        => solver.CountSolutions(
            maxSolutions: benchCase.MaxCount,
            multiThread: benchCase.MultiThread);

    private static double ElapsedMilliseconds(long start, long finish)
        => (finish - start) * 1000.0 / Stopwatch.Frequency;

    private static void ValidateRequest(BenchRequest request)
    {
        if (request.Cases.Length == 0)
        {
            throw new ArgumentException("At least one benchmark case is required.");
        }
        if (request.WarmupRounds < 0 || request.Repeats <= 0)
        {
            throw new ArgumentException("Warm-up rounds must be non-negative and repeats positive.");
        }

        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (BenchCase benchCase in request.Cases)
        {
            if (string.IsNullOrWhiteSpace(benchCase.Name) || !names.Add(benchCase.Name))
            {
                throw new ArgumentException("Every case must have a unique non-empty name.");
            }
            if (benchCase.Blank != 9 || benchCase.Op != "count" || benchCase.MaxCount != 2 || benchCase.MultiThread)
            {
                throw new ArgumentException(
                    $"{benchCase.Name}: this harness only accepts single-threaded blank-9 count-to-two cases.");
            }
            if (benchCase.Constraints.Length == 0 ||
                benchCase.Constraints.Any(c => !c.StartsWith("thermo:", StringComparison.Ordinal)))
            {
                throw new ArgumentException(
                    $"{benchCase.Name}: every constraint must be a thermometer.");
            }
        }
    }

    private static void ValidateObserved(BenchCase benchCase, long observed)
    {
        if (observed != benchCase.Expected)
        {
            throw new InvalidOperationException(
                $"{benchCase.Name}: observed {observed}, expected {benchCase.Expected}.");
        }
    }
}
