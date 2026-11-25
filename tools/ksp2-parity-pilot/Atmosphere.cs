using System.Globalization;
using System.IO;
using KSP.Game;

namespace OpenSpaceExplorer.Validation
{
    // Air on every body (docs/ksp2-side-by-side-gaps.md T2/T11): for each body with an atmosphere, KSP2's own
    // CelestialBodyComponent.GetPressure / GetTemperature / GetDensity and AeroUtilities.GetSoundSpeed (what the
    // flight telemetry, drag and heating read) at 1,000 heights from sea level to the top of the air, and the
    // body's atmosphere constants. Written to atmosphere.csv and atmosphere-bodies.csv; nothing is flown.
    public static class AtmosphereScenario
    {
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static string F(double x) => x.ToString("R", Inv);

        public static string Run(string dir)
        {
            var g = GameManager.Instance.Game;
            int n = 0;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "atmosphere.csv"), false))
            using (var b = new StreamWriter(Path.Combine(dir, "out", "atmosphere-bodies.csv"), false))
            {
                w.WriteLine("body,alt_m,pressure_kpa,temperature_k,density,sound_mps");
                b.WriteLine("body,depth_m,molar_mass,adiabatic,pressure_asl_kpa,temperature_asl_k,density_asl");
                foreach (var body in g.UniverseModel.GetAllCelestialBodies())
                {
                    if (body == null || !body.hasAtmosphere || body.atmosphereDepth <= 0) continue;
                    double depth = body.atmosphereDepth;
                    double p0 = body.GetPressure(0), t0 = body.GetTemperature(0);
                    b.WriteLine(string.Join(",", body.bodyName, F(depth), F(body.atmosphereMolarMass), F(body.atmosphereAdiabaticIndex), F(p0), F(t0), F(body.GetDensity(p0, t0))));
                    for (int k = 0; k <= 1000; k++)
                    {
                        double alt = depth * k / 1000.0;
                        double p = body.GetPressure(alt), t = body.GetTemperature(alt), rho = body.GetDensity(p, t);
                        double c = global::AeroUtilities.GetSoundSpeed(body.atmosphereAdiabaticIndex, p, rho);
                        w.WriteLine(string.Join(",", body.bodyName, F(alt), F(p), F(t), F(rho), F(c)));
                    }
                    n++;
                }
            }
            return "atmosphere: " + n + " bodies with air";
        }
    }
}
