"use client";
import * as MenuParts from "@ui-sdk/react/menu";
import * as DialogParts from "@ui-sdk/react/dialog";
import * as TabParts from "@ui-sdk/react/tabs";
import * as CollapsibleParts from "@ui-sdk/react/collapsible";

export const Menu = { Root: MenuParts.Menu, Trigger: MenuParts.MenuTrigger, Popup: MenuParts.MenuContent, Group: MenuParts.MenuGroup, GroupLabel: MenuParts.MenuGroupLabel, Item: MenuParts.MenuItem, CheckboxItem: MenuParts.MenuCheckboxItem, CheckboxItemIndicator: MenuParts.MenuItemIndicator };
export const Dialog = { Root: DialogParts.Dialog, Popup: DialogParts.DialogContent, Backdrop: DialogParts.DialogBackdrop, Title: DialogParts.DialogTitle, Description: DialogParts.DialogDescription, Close: DialogParts.DialogClose };
export const Tabs = { Root: TabParts.Tabs, List: TabParts.TabsList, Tab: TabParts.TabsTab, Panel: TabParts.TabsPanel };
export const Collapsible = { Root: CollapsibleParts.Collapsible, Trigger: CollapsibleParts.CollapsibleTrigger, Panel: CollapsibleParts.CollapsiblePanel };
