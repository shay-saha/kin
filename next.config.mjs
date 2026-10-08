const backend = (process.env.KIN_BACKEND_URL || `http://127.0.0.1:${process.env.KIN_BRAIN_PORT || "8787"}`).replace(/\/$/, "");

const nextConfig = {
  async rewrites() {
    return {
      beforeFiles: [
        { source: "/api/:path*", destination: `${backend}/api/:path*` },
        { source: "/auth/callback", destination: `${backend}/auth/callback` },
      ],
    };
  },
};

export default nextConfig;
