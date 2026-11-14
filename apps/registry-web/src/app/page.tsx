import React from "react";

interface PackageSummary {
  scope: string;
  name: string;
  latest_version: string | null;
  description: string | null;
  downloads: number;
}

async function fetchPackages(): Promise<PackageSummary[]> {
  const api = process.env.NEXT_PUBLIC_REGISTRY_API_URL || "http://localhost:8080";
  try {
    const res = await fetch(`${api}/api/v1/packages?per_page=20`, { cache: "no-store" });
    if (!res.ok) return [];
    const data = await res.json();
    return data.items || [];
  } catch {
    return [];
  }
}

export default async function RegistryHomePage() {
  const packages = await fetchPackages();

  return (
    <div>
      <section style={{ marginBottom: "48px" }}>
        <h1 style={{ fontSize: "32px", fontWeight: 600, letterSpacing: "-0.03em", marginBottom: "12px" }}>
          Intelligence, unbundled.
        </h1>
        <p style={{ color: "#57534E", maxWidth: "600px", marginBottom: "24px" }}>
          Discover and publish typed, deterministic Block packages for composable AI systems.
        </p>
        <form action="/search" method="get" style={{ display: "flex", gap: "8px", maxWidth: "480px" }}>
          <input
            type="text"
            name="q"
            placeholder="Search @scope/package..."
            style={{
              flex: 1,
              padding: "10px 14px",
              border: "1px solid #D6D3D1",
              backgroundColor: "#FFFFFF",
              fontFamily: "ui-monospace, monospace",
              fontSize: "14px",
            }}
          />
          <button
            type="submit"
            style={{
              padding: "10px 20px",
              backgroundColor: "#1C1917",
              color: "#FAF8F5",
              border: "none",
              cursor: "pointer",
              fontSize: "14px",
            }}
          >
            Search
          </button>
        </form>
      </section>

      <section>
        <h2 style={{ fontSize: "14px", textTransform: "uppercase", letterSpacing: "0.08em", color: "#78716C", borderBottom: "1px solid #E7E5E4", paddingBottom: "8px" }}>
          Published Packages
        </h2>
        {packages.length === 0 ? (
          <p style={{ color: "#78716C", fontFamily: "ui-monospace, monospace", fontSize: "13px", marginTop: "16px" }}>
            No packages published yet. Run `sub publish` to publish your first package.
          </p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
            {packages.map((pkg) => (
              <li
                key={`${pkg.scope}/${pkg.name}`}
                style={{
                  borderBottom: "1px solid #E7E5E4",
                  padding: "16px 0",
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "baseline",
                }}
              >
                <div>
                  <a
                    href={`/packages/${pkg.scope}/${pkg.name}`}
                    style={{
                      fontFamily: "ui-monospace, monospace",
                      fontSize: "15px",
                      fontWeight: 600,
                      color: "#1C1917",
                      textDecoration: "none",
                    }}
                  >
                    @{pkg.scope}/{pkg.name}
                  </a>
                  {pkg.description && (
                    <p style={{ margin: "4px 0 0", fontSize: "14px", color: "#57534E" }}>
                      {pkg.description}
                    </p>
                  )}
                </div>
                <div style={{ fontFamily: "ui-monospace, monospace", fontSize: "13px", color: "#78716C" }}>
                  v{pkg.latest_version ?? "0.0.0"} · {pkg.downloads} downloads
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
