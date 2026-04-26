import React from "react";
import ReactDOM from "react-dom/client";
import {
  MutationCache,
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from "@tanstack/react-query";
import { ReactQueryDevtools } from "@tanstack/react-query-devtools";
import App from "./App";
import { pushError } from "./toast";
import "./styles.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
    },
  },
  queryCache: new QueryCache({
    onError: (error, query) => {
      const title =
        (query.meta?.errorTitle as string | undefined) ?? "Request failed";
      pushError(title, error);
    },
  }),
  mutationCache: new MutationCache({
    onError: (error, _vars, _ctx, mutation) => {
      const title =
        (mutation.options.meta?.errorTitle as string | undefined) ??
        "Request failed";
      pushError(title, error);
    },
  }),
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
      <ReactQueryDevtools initialIsOpen={false} buttonPosition="bottom-right" />
    </QueryClientProvider>
  </React.StrictMode>,
);
