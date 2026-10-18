// TEST PAGE, never in the production dist: its route exists only in `vite build --mode perf` (src/router.tsx). It mounts every overlay
// primitive of the app, each behind a button, so the CSP sweep (e2e/csp/csp.spec.ts) can open them all under the
// production header and fail on any violation. The overlays create <style> elements (Radix's scroll lock), position with
// floating-ui (style props, which are CSSOM) and portal into <body>; this is where an inline-style or Trusted Types break
// would show.
import { useState } from "react";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { ProblemDialog } from "@/components/app/ProblemDialog";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { NativeSelect } from "@/components/ui/native-select";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
export function PrimitivesPage() {
  const [consequence, setConsequence] = useState(false);
  const [destructive, setDestructive] = useState(false);
  const [problem, setProblem] = useState<"conflict" | "locked" | "approval" | null>(null);
  const [palette, setPalette] = useState(false);
  return (
    <TooltipProvider>
      <div className="flex flex-col gap-4 p-6">
        <h1 className="text-lg font-semibold">Primitives</h1>

        <div className="flex flex-wrap items-center gap-2">
          <Dialog>
            <DialogTrigger asChild>
              <Button>Open dialog</Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>A dialog</DialogTitle>
                <DialogDescription>A modal with a scroll lock.</DialogDescription>
              </DialogHeader>
            </DialogContent>
          </Dialog>

          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="secondary">Open alert dialog</Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>An alert dialog</AlertDialogTitle>
                <AlertDialogDescription>It needs an answer.</AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction>Confirm change</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>

          <Button onClick={() => setConsequence(true)}>Open consequence dialog</Button>
          <Button variant="destructive" onClick={() => setDestructive(true)}>
            Open destructive confirm
          </Button>
          <Button variant="secondary" onClick={() => setProblem("conflict")}>
            Open conflict
          </Button>
          <Button variant="secondary" onClick={() => setProblem("locked")}>
            Open locked
          </Button>
          <Button variant="secondary" onClick={() => setProblem("approval")}>
            Open sent for approval
          </Button>
          <Button variant="secondary" onClick={() => setPalette(true)}>
            Open palette
          </Button>

          <Popover>
            <PopoverTrigger asChild>
              <Button variant="secondary">Open popover</Button>
            </PopoverTrigger>
            <PopoverContent aria-label="Example popover">A popover, positioned by floating-ui.</PopoverContent>
          </Popover>

          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="secondary">Open menu</Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuLabel>Account</DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem>Theme</DropdownMenuItem>
              <DropdownMenuItem destructive>Sign out</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost">Hover for tooltip</Button>
            </TooltipTrigger>
            <TooltipContent>A tooltip</TooltipContent>
          </Tooltip>
        </div>

        <Tabs defaultValue="one">
          <TabsList>
            <TabsTrigger value="one">One</TabsTrigger>
            <TabsTrigger value="two">Two</TabsTrigger>
          </TabsList>
          <TabsContent value="one">First tab</TabsContent>
          <TabsContent value="two">Second tab</TabsContent>
        </Tabs>

        <ToggleGroup type="single" defaultValue="split" aria-label="View">
          <ToggleGroupItem value="split">Split</ToggleGroupItem>
          <ToggleGroupItem value="unified">Unified</ToggleGroupItem>
        </ToggleGroup>

        <NativeSelect aria-label="Tier" defaultValue="sit">
          <option value="sit">SIT</option>
          <option value="presit">Pre-SIT</option>
        </NativeSelect>

        <div className="h-32 border border-border">
          <ResizablePanelGroup orientation="horizontal">
            <ResizablePanel defaultSize={30}>Tree</ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={70}>Content</ResizablePanel>
          </ResizablePanelGroup>
        </div>

        <Command aria-label="Inline command list" className="border border-border">
          <CommandInput placeholder="Filter" />
          <CommandList>
            <CommandEmpty>No results.</CommandEmpty>
            <CommandGroup heading="Swimlanes">
              <CommandItem>sit1</CommandItem>
              <CommandItem>sit2</CommandItem>
            </CommandGroup>
          </CommandList>
        </Command>
      </div>

      <ConsequenceDialog open={consequence} onOpenChange={setConsequence} title="Raise a pull request" consequence="2 files will be copied into the base repository." actionLabel="Raise PR" onConfirm={() => setConsequence(false)} />
      <DestructiveConfirm open={destructive} onOpenChange={setDestructive} title="Delete file" consequence="The file is removed from the swimlane." confirmValue="core-sit1.yml" actionLabel="Delete file" onConfirm={() => setDestructive(false)} />
      <ProblemDialog open={problem !== null} onOpenChange={(open) => !open && setProblem(null)} variant={problem ?? "conflict"} requestId="req-test-0001" />
      <CommandDialog open={palette} onOpenChange={setPalette} title="Command palette" description="Search swimlanes, docs and settings.">
        <CommandInput placeholder="Type a command" />
        <CommandList>
          <CommandEmpty>No results.</CommandEmpty>
          <CommandGroup heading="Swimlanes">
            <CommandItem>sit1</CommandItem>
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </TooltipProvider>
  );
}
