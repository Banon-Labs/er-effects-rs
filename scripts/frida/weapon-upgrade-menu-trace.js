const moduleName = "eldenring.exe";
const base = Process.getModuleByName(moduleName).base;

const RVAS = {
	currentOpenMenu: 0x458baec,
	openEnhanceShop: 0xe9de00,
	giveItems: 0x5605b0,
	armamentRowTransform: 0x98f850,
	equipInventoryInsertItem: 0x24d910,
	getReinforcement: 0x672740,
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
	menuGaitemMetadataPtr: 0x58,
	metadataAstruct96: 0x8,
	astruct96ItemCategory: 0x17,
};

const LIMITS = {
	rowTransform: 200,
	insertItem: 80,
	getReinforcement: 80,
};

const counts = {};

function ptrAt(rva) {
	return base.add(rva);
}

function count(name) {
	const next = (counts[name] || 0) + 1;
	counts[name] = next;
	return next;
}

function safeU8(addr) {
	try {
		return addr.readU8();
	} catch (_) {
		return null;
	}
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

function hexPtr(value) {
	if (value === null || value === undefined) return null;
	return value.toString();
}

function currentOpenMenu() {
	return safeU32(ptrAt(RVAS.currentOpenMenu));
}

function readMenuGaitem(row) {
	if (row.isNull()) return { ptr: row.toString(), null: true };
	const itemId = safeU32(row.add(OFFSETS.menuGaitemItemId));
	const itemType = safeU32(row.add(OFFSETS.menuGaitemItemType));
	let itemCategory = null;
	const metadata = safePtr(row.add(OFFSETS.menuGaitemMetadataPtr));
	const astruct96 =
		metadata === null || metadata.isNull()
			? null
			: safePtr(metadata.add(OFFSETS.metadataAstruct96));
	if (astruct96 !== null && !astruct96.isNull()) {
		itemCategory = safeU8(astruct96.add(OFFSETS.astruct96ItemCategory));
	}
	return {
		ptr: row.toString(),
		item_id: itemId,
		item_id_hex: itemId === null ? null : `0x${itemId.toString(16)}`,
		item_type: itemType,
		item_type_hex: itemType === null ? null : `0x${itemType.toString(16)}`,
		item_id_category_hex:
			itemId === null ? null : `0x${(itemId & 0xf0000000).toString(16)}`,
		metadata: hexPtr(metadata),
		astruct96: hexPtr(astruct96),
		item_category: itemCategory,
		open_menu: currentOpenMenu(),
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
		items.push({
			index: i,
			item_id: safeI32(entry.add(OFFSETS.itemLotItemId)),
			quantity: safeI32(entry.add(OFFSETS.itemLotQuantity)),
			upgrade: safeI32(entry.add(OFFSETS.itemLotUpgrade)),
			gem_id: safeI32(entry.add(OFFSETS.itemLotGemId)),
		});
	}
	return { ptr: listPtr.toString(), count: countValue, items };
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

attach("OpenEnhanceShop", RVAS.openEnhanceShop, {
	onEnter(args) {
		this.typeArg = args[1].toInt32();
		emit("open_enhance_shop_enter", {
			out: args[0].toString(),
			type: this.typeArg,
			current_open_menu_before: currentOpenMenu(),
		});
	},
	onLeave(retval) {
		emit("open_enhance_shop_leave", {
			retval: retval.toString(),
			type: this.typeArg,
			current_open_menu_after: currentOpenMenu(),
		});
	},
});

attach("GiveItems", RVAS.giveItems, {
	onEnter(args) {
		emit("give_items_enter", {
			map_item_man: args[0].toString(),
			item_lot: readItemLot(args[1]),
			status: args[2].toString(),
			unknown_u32: args[3].toUInt32(),
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

attach("ArmamentRowTransform", RVAS.armamentRowTransform, {
	onEnter(args) {
		const n = count("rowTransform");
		this.n = n;
		this.row = args[0];
		if (n <= LIMITS.rowTransform) {
			emit("armament_row_transform_enter", { n, row: readMenuGaitem(args[0]) });
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.rowTransform) {
			emit("armament_row_transform_leave", {
				n: this.n,
				in_row: this.row.toString(),
				retval: retval.toString(),
				row: readMenuGaitem(retval),
			});
		}
	},
});

attach("EquipInventoryData::InsertItem", RVAS.equipInventoryInsertItem, {
	onEnter(args) {
		const n = count("insertItem");
		this.n = n;
		if (n <= LIMITS.insertItem) {
			emit("insert_item_enter", {
				n,
				this_ptr: args[0].toString(),
				gaitem_handle_ptr: args[1].toString(),
				arg2: args[2].toString(),
				arg3_i32: args[3].toInt32(),
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

attach("GetReinforcement", RVAS.getReinforcement, {
	onEnter(args) {
		const n = count("getReinforcement");
		this.n = n;
		if (n <= LIMITS.getReinforcement) {
			emit("get_reinforcement_enter", {
				n,
				lookup: args[0].toString(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
	onLeave(retval) {
		if (this.n <= LIMITS.getReinforcement) {
			emit("get_reinforcement_leave", {
				n: this.n,
				retval: retval.toInt32(),
				current_open_menu: currentOpenMenu(),
			});
		}
	},
});

emit("ready", { current_open_menu: currentOpenMenu() });
