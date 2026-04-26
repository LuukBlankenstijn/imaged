import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { DashboardService } from "@imaged/gen/v1/dashboard/dashboard_pb";

const transport = createGrpcWebTransport({
  baseUrl: "/api",
});

export const dashboardClient = createClient(DashboardService, transport);
