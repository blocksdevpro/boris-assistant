import type { NextConfig } from "next";

const windowsInstaller =
  "https://github.com/blocksdevpro/boris-assistant/releases/download/v1.1.0/Boris_1.1.0_x64-setup.exe";

const nextConfig: NextConfig = {
  async redirects() {
    return [
      {
        source: "/download",
        destination: windowsInstaller,
        permanent: false,
      },
    ];
  },
};

export default nextConfig;
