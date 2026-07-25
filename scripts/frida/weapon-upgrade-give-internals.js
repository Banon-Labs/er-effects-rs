const moduleName = "eldenring.exe";
const base = Process.getModuleByName(moduleName).base;

const RVAS = {
	currentOpenMenu: 0x458baec,
	giveItems: 0x5605b0,
	isItemIdParamExists: 0x675680,
	validateItemAcquisition: 0x55ec90,
	buildGaitemHandle: 0x55f020,
	addViaEquipGameData: 0x55edb0,
	addInventoryEquip: 0x246480,
	insertItem: 0x24cfd0,
	changeAmountOrWrapper: 0x24d910,
	getItemIdByIndex: 0x247cf0,
	getItemQuantity: 0x247f20,
	getGaItemHandleByIndex: 0x24c7b0,
	buildMenuGaitemByIndex: 0x847a20,
	inventoryListSource: 0x84d3f0,
	selectedConfirm: 0x98df10,
	selectedItemExistsInInventory: 0x785be0,
	openEnhanceShop: 0xe9de00,
	armamentRowTransform: 0x98f850,
};

const OFFSETS = {
	itemLotCount: 0x0,
	itemLotItems: 0x4,
	itemLotEntrySize: 0x10,
	itemLotItemId: 0x0,
	itemLotQuantity: 0x4,
	itemLotUpgrade: 0x8,
	itemLotGemId: 0xc,
	menuGaitemItemId: 0x4c,
	menuGaitemItemType: 0x54,
};

const counts = {};
const LIMITS = {
	itemParamExists: 20,
	validate: 20,
	buildHandle: 20,
	addViaEquip: 20,
	addInventoryEquip: 20,
	insertItem: 40,
	wrapper: 40,
	byIndex: 160,
	rowBuilder: 160,
	selectedConfirm: 40,
	selectedExists: 80,
	row: 80,
};

function ptrAt(rva) {
	return base.add(rva);
}

function count(name) {
	const next = (counts[name] || 0) + 1;
	counts[name] = next;
	return next;
}

function safeU32(addr) {
	try {
		return addr.readU32();
	} catch (_) {
		return null;
	}
}

function safeI32(addr) {
	try {
		return addr.readS32();
	} catch (_) {
		return null;
	}
}

function safePtr(addr) {
	try {
		return addr.readPointer();
	} catch (_) {
		return null;
	}
}

function currentOpenMenu() {
	return safeU32(ptrAt(RVAS.currentOpenMenu));
}

function hexItem(value) {
	if (value === null || value === undefined) return null;
	return `0x${(value >>> 0).toString(16)}`;
}

function readItemId(ptr) {
	const itemId = safeU32(ptr);
	return {
		ptr: ptr.toString(),
		item_id: itemId,
		item_id_hex: hexItem(itemId),
		item_id_category_hex: itemId === null ? null : hexItem(itemId & 0xf0000000),
	};
}

function readHandle(ptr) {
	return {
		ptr: ptr.toString(),
		first_u32: safeU32(ptr),
		second_u32: safeU32(ptr.add(4)),
		first_hex: hexItem(safeU32(ptr)),
		second_hex: hexItem(safeU32(ptr.add(4))),
	};
}

function readItemLot(listPtr) {
	if (listPtr.isNull()) return { ptr: listPtr.toString(), null: true };
	const countValue = safeU32(listPtr.add(OFFSETS.itemLotCount));
	const countClamped = countValue === null ? 0 : Math.min(countValue, 10);
	const items = [];
	for (let i = 0; i < countClamped; i++) {
		const entry = listPtr.add(
			OFFSETS.itemLotItems + i * OFFSETS.itemLotEntrySize,
		);
		const itemId = safeI32(entry.add(OFFSETS.itemLotItemId));
		items.push({
			index: i,
			item_id: itemId,
			item_id_hex: hexItem(itemId),
			quantity: safeI32(entry.add(OFFSETS.itemLotQuantity)),
			upgrade: safeI32(entry.add(OFFSETS.itemLotUpgrade)),
			gem_id: safeI32(entry.add(OFFSETS.itemLotGemId)),
		});
	}
	return { ptr: listPtr.toString(), count: countValue, items };
}

function indexIsInteresting(index) {
	return index === 2252 || index < 16;
}

function readMenuGaitem(row) {
	if (row.isNull()) return { ptr: row.toString(), null: true };
	const itemId = safeU32(row.add(OFFSETS.menuGaitemItemId));
	return {
		ptr: row.toString(),
		item_id: itemId,
		item_id_hex: hexItem(itemId),
		item_type: safeU32(row.add(OFFSETS.menuGaitemItemType)),
		item_id_category_hex: itemId === null ? null : hexItem(itemId & 0xf0000000),
		open_menu: currentOpenMenu(),
	};
}

function emit(kind, fields) {
	send(
		Object.assign({ kind, base: base.toString(), tick_ms: Date.now() }, fields),
	);
}

function attach(name, rva, callbacks) {
	const address = ptrAt(rva);
	emit("hook_install", {
		name,
		address: address.toString(),
		rva: `0x${rva.toString(16)}`,
	});
	Interceptor.attach(address, callbacks);
}

attach("GiveItems", RVAS.giveItems, {
	onEnter(args) {
		this.list = args[1];
		emit("give_items_enter", {
			map_item_man: args[0].toString(),
			item_lot: readItemLot(args[1]),
			status: args[2].toString(),
			show_ok_prompt: args[3].toUInt32(),
			current_open_menu: currentOpenMenu(),
		});
	},
	onLeave(retval) {
		emit("give_items_leave", {
			retval: retval.toString(),
			current_open_menu: currentOpenMenu(),
		});
	},
});

attach("IsItemIdParamExists", RVAS.isItemIdParamExists, {
	onEnter(args) {
		const n = count("itemParamExists");
		this.n = n;
		if (n <= LIMITS.itemParamExists) {
			emit("item_param_exists_enter", {
				n,
				item: readItemId(args[0]),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.itemParamExists) {
			emit("item_param_exists_leave", {
				n: this.n,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("ValidateItemAcquisition", RVAS.validateItemAcquisition, {
	onEnter(args) {
		const n = count("validate");
		this.n = n;
		this.actualOut = args[2];
		if (n <= LIMITS.validate) {
			emit("validate_enter", {
				n,
				item: readItemId(args[0]),
				requested_amount: args[1].toInt32(),
				actual_out_before: safeI32(args[2]),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.validate) {
			emit("validate_leave", {
				n: this.n,
				retval: retval.toInt32(),
				actual_out_after: safeI32(this.actualOut),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("BuildGaItemHandle", RVAS.buildGaitemHandle, {
	onEnter(args) {
		const n = count("buildHandle");
		this.n = n;
		this.out = args[1];
		if (n <= LIMITS.buildHandle) {
			emit("build_handle_enter", {
				n,
				out: args[1].toString(),
				item: readItemId(args[2]),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.buildHandle) {
			emit("build_handle_leave", {
				n: this.n,
				retval: retval.toString(),
				out_handle: readHandle(this.out),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("AddViaEquipGameData", RVAS.addViaEquipGameData, {
	onEnter(args) {
		const n = count("addViaEquip");
		this.n = n;
		if (n <= LIMITS.addViaEquip) {
			emit("add_via_equip_enter", {
				n,
				handle: readHandle(args[1]),
				amount: args[2].toInt32(),
				durability: args[3].toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.addViaEquip) {
			emit("add_via_equip_leave", {
				n: this.n,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("AddInventoryEquip", RVAS.addInventoryEquip, {
	onEnter(args) {
		const n = count("addInventoryEquip");
		this.n = n;
		this.equipGameData = args[0];
		if (n <= LIMITS.addInventoryEquip) {
			emit("add_inventory_equip_enter", {
				n,
				equip_game_data: args[0].toString(),
				handle: readHandle(args[1]),
				amount: args[2].toInt32(),
				update_trophy: args[3].toInt32(),
				update_auto_equip: args[4].toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.addInventoryEquip) {
			emit("add_inventory_equip_leave", {
				n: this.n,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("EquipInventoryData::InsertItem", RVAS.insertItem, {
	onEnter(args) {
		const n = count("insertItem");
		this.n = n;
		if (n <= LIMITS.insertItem) {
			emit("insert_item_enter", {
				n,
				equip_inventory: args[0].toString(),
				handle: readHandle(args[1]),
				amount: args[2].toInt32(),
				durability: args[3].toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.insertItem) {
			emit("insert_item_leave", {
				n: this.n,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach(
	"EquipInventoryData::FUN_14024d910_wrapper",
	RVAS.changeAmountOrWrapper,
	{
		onEnter(args) {
			const n = count("wrapper");
			this.n = n;
			if (n <= LIMITS.wrapper) {
				emit("insert_wrapper_enter", {
					n,
					equip_inventory: args[0].toString(),
					handle: readHandle(args[1]),
					arg2: args[2].toInt32(),
					arg3: args[3].toInt32(),
					current_open_menu: currentOpenMenu(),
				});
			}
		},
		onLeave(retval) {
			if (this.n <= LIMITS.wrapper) {
				emit("insert_wrapper_leave", {
					n: this.n,
					retval: retval.toString(),
					current_open_menu: currentOpenMenu(),
				});
			}
		},
	},
);

attach("EquipGameData::GetItemIdByIndex", RVAS.getItemIdByIndex, {
	onEnter(args) {
		const n = count("byIndex");
		this.n = n;
		this.index = args[2].toUInt32();
		this.out = args[1];
		if (n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
			emit("get_item_id_by_index_enter", {
				n,
				index: this.index,
				out: args[1].toString(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
			emit("get_item_id_by_index_leave", {
				n: this.n,
				index: this.index,
				retval: retval.toString(),
				out_item_id: safeI32(this.out),
				out_item_id_hex: hexItem(safeI32(this.out)),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("EquipGameData::GetItemQuantity", RVAS.getItemQuantity, {
	onEnter(args) {
		const n = count("quantity");
		this.n = n;
		this.index = args[1].toUInt32();
		if (n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
			emit("get_item_quantity_enter", {
				n,
				index: this.index,
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
			emit("get_item_quantity_leave", {
				n: this.n,
				index: this.index,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach(
	"EquipInventoryData::GetGaItemHandleByIndex",
	RVAS.getGaItemHandleByIndex,
	{
		onEnter(args) {
			const n = count("handleByIndex");
			this.n = n;
			this.out = args[1];
			this.index = args[2].toUInt32();
			if (n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
				emit("get_handle_by_index_enter", {
					n,
					index: this.index,
					out: args[1].toString(),
					current_open_menu: currentOpenMenu(),
				});
			}
		},
		onLeave(retval) {
			if (this.n <= LIMITS.byIndex || indexIsInteresting(this.index)) {
				emit("get_handle_by_index_leave", {
					n: this.n,
					index: this.index,
					retval: retval.toString(),
					out_handle: readHandle(this.out),
					current_open_menu: currentOpenMenu(),
				});
			}
		},
	},
);

attach("BuildMenuGaitemByIndex", RVAS.buildMenuGaitemByIndex, {
	onEnter(args) {
		const n = count("rowBuilder");
		this.n = n;
		this.index = args[1].toUInt32();
		if (n <= LIMITS.rowBuilder || indexIsInteresting(this.index)) {
			emit("build_menu_gaitem_by_index_enter", {
				n,
				index: this.index,
				out: args[0].toString(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.rowBuilder || indexIsInteresting(this.index)) {
			emit("build_menu_gaitem_by_index_leave", {
				n: this.n,
				index: this.index,
				retval: retval.toString(),
				row: readMenuGaitem(retval),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("InventoryListSource", RVAS.inventoryListSource, {
	onEnter(args) {
		emit("inventory_list_source_enter", {
			out: args[0].toString(),
			current_open_menu: currentOpenMenu(),
		});
	},
	onLeave(retval) {
		emit("inventory_list_source_leave", {
			retval: retval.toString(),
			current_open_menu: currentOpenMenu(),
		});
	},
});

attach("SelectedConfirmDialogPath", RVAS.selectedConfirm, {
	onEnter(args) {
		const n = count("selectedConfirm");
		this.n = n;
		if (n <= LIMITS.selectedConfirm) {
			emit("selected_confirm_enter", {
				n,
				param_1: args[0].toString(),
				param_2: args[1].toString(),
				selected: readMenuGaitem(args[2]),
				param_4: args[3].toString(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.selectedConfirm) {
			emit("selected_confirm_leave", {
				n: this.n,
				retval: retval.toString(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("SelectedItemExistsInInventory", RVAS.selectedItemExistsInInventory, {
	onEnter(args) {
		const n = count("selectedExists");
		this.n = n;
		this.item = readItemId(args[0]);
		if (n <= LIMITS.selectedExists) {
			emit("selected_exists_enter", {
				n,
				item: this.item,
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.selectedExists) {
			emit("selected_exists_leave", {
				n: this.n,
				item: this.item,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

attach("OpenEnhanceShop", RVAS.openEnhanceShop, {
	onEnter(args) {
		this.typeArg = args[1].toInt32();
		emit("open_enhance_shop_enter", {
			type: this.typeArg,
			current_open_menu_before: currentOpenMenu(),
		});
	},
	onLeave(retval) {
		emit("open_enhance_shop_leave", {
			type: this.typeArg,
			retval: retval.toString(),
			current_open_menu_after: currentOpenMenu(),
		});
	},
});

attach("ArmamentRowTransform", RVAS.armamentRowTransform, {
	onEnter(args) {
		const n = count("row");
		this.n = n;
		if (n <= LIMITS.row) {
			emit("armament_row_transform_enter", { n, row: readMenuGaitem(args[0]) });
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.row) {
			emit("armament_row_transform_leave", {
				n: this.n,
				retval: retval.toString(),
				row: readMenuGaitem(retval),
			});
		}
	},
});

emit("ready", { current_open_menu: currentOpenMenu(), mode: "give_internals" });
