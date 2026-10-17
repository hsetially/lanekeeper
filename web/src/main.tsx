import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { installNonce } from "@/lib/csp/nonce";
import { createQueryClient } from "@/lib/query/client";
import { createAppRouter } from "./router";
import "./styles/tailwind.css";

// The nonce first: libraries that make a <style> later (the scroll lock of the dialogs) read it from the window.
installNonce();

const queryClient = createQueryClient();
const router = createAppRouter({ queryClient });

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>,
  );
}
