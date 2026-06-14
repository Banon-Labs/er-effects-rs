#!/usr/bin/env python3
"""Emit deterministic static RE evidence for the Elden Ring autoload path.

This script is intentionally read-only: it parses the local eldenring.exe PE
image and records exact address/RVA relationships around the menu task wrappers
and GameMan save/load scheduler helpers used by the autoresearch benchmark.
"""
from __future__ import annotations

import hashlib
import json
import os
import struct
import sys
from pathlib import Path
from typing import Any

DEFAULT_EXE = Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
IMAGE_BASE_FALLBACK = 0x140000000

TARGETS = {
    "menu_continue_wrapper": 0x14082BAC0,
    "menu_new_or_load_wrapper": 0x14082BA80,
    "menu_other_load_wrapper": 0x14082BB00,
    "menu_task_update_wrapper": 0x14082A0F0,
    "move_map_save_load_dispatch": 0x140AFB880,
    "map_load_67bc10": 0x14067BC10,
    "save_load_pump_delta": 0x1406794B0,
    "save_load_pump_default": 0x140679510,
    "requested_slot_validation": 0x14067B200,
    "post_requested_slot_67a320": 0x14067A320,
    "set_save_slot_67a810": 0x14067A810,
    "slot_reset_state_table_init_0a4f50": 0x1400A4F50,
    "slot_reset_state_table_global_43d71580": 0x143D71580,
    "slot_reset_parent_loop_b0bd60": 0x140B0BD60,
    "slot_reset_title_parent_ctor_b0b020": 0x140B0B020,
    "slot_reset_title_parent_base_ctor_b0b0d0": 0x140B0B0D0,
    "slot_reset_title_step_ctor_b0b1c0": 0x140B0B1C0,
    "slot_reset_title_queue_state_table_init_0a4c90": 0x1400A4C90,
    "slot_reset_title_queue_state_table_global_43d71340": 0x143D71340,
    "slot_reset_title_queue_seed_b0a4a0": 0x140B0A4A0,
    "slot_reset_title_queue_pump_b0a5e0": 0x140B0A5E0,
    "slot_reset_title_queue_ingame_init_b0a1f0": 0x140B0A1F0,
    "slot_reset_title_queue_ingame_standby_b0a430": 0x140B0A430,
    "slot_reset_title_queue_ingame_b0a1b0": 0x140B0A1B0,
    "slot_reset_title_queue_end_b0a170": 0x140B0A170,
    "slot_reset_title_queue_advance_b0a980": 0x140B0A980,
    "slot_reset_title_queue_source_81f7e0": 0x14081F7E0,
    "slot_reset_title_queue_take_7a9560": 0x1407A9560,
    "slot_reset_title_queue_state_set_b0aa90": 0x140B0AA90,
    "slot_reset_title_queue_stream_validate_71fd60": 0x14071FD60,
    "slot_reset_menu_job_wait_handler_b0d400": 0x140B0D400,
    "slot_reset_play_game_handler_b0d5b0": 0x140B0D5B0,
    "slot_reset_play_game_tail_b0d850": 0x140B0D850,
    "slot_reset_end_flow_wait_handler_b0ccc0": 0x140B0CCC0,
    "slot_reset_end_flow_wait_probe_eb5530": 0x140EB5530,
    "slot_reset_end_flow_reset_67ae90": 0x14067AE90,
    "game_man_global_43d69918": 0x143D69918,
    "game_man_b5e_getter_67a1b0": 0x14067A1B0,
    "game_man_b5e_bulk_clear_679830": 0x140679830,
    "slot_reset_to_menu_job_wait_helper_b0e530": 0x140B0E530,
    "slot_reset_title_beginlogo_owner_e0_builder_81f180": 0x14081F180,
    "slot_reset_title_begintitle_owner138_e0_builder_81f9f0": 0x14081F9F0,
    "slot_reset_title_begintitle_bool_wrapper_b0c180": 0x140B0C180,
    "slot_reset_title_accept_input_condition_builder_7acb00": 0x1407ACB00,
    "slot_reset_title_accept_input_alloc_seed_7a72a0": 0x1407A72A0,
    "slot_reset_title_accept_input_node_ctor_7a6f20": 0x1407A6F20,
    "slot_reset_title_accept_input_node_update_7ad1c0": 0x1407AD1C0,
    "slot_reset_title_accept_input_manager_state_765f20": 0x140765F20,
    "slot_reset_title_accept_input_manager_shutdown_765fa0": 0x140765FA0,
    "slot_reset_title_accept_input_manager_init_766010": 0x140766010,
    "slot_reset_title_accept_input_manager_queue_setup_7660f0": 0x1407660F0,
    "slot_reset_title_accept_input_manager_global_43d6b7b0": 0x143D6B7B0,
    "slot_reset_title_accept_input_manager_singleton_43d6b880": 0x143D6B880,
    "slot_reset_title_accept_input_manager_allocator_43d87350": 0x143D87350,
    "slot_reset_title_accept_input_temp_base_vtable_aa9808": 0x142AA9808,
    "slot_reset_title_accept_input_temp_active_vtable_aa9840": 0x142AA9840,
    "slot_reset_title_accept_input_node_vtable_aa97e8": 0x142AA97E8,
    "slot_reset_title_accept_input_temp_clone_7ad6c0": 0x1407AD6C0,
    "slot_reset_title_accept_input_temp_callback_7ad810": 0x1407AD810,
    "slot_reset_title_accept_input_temp_active_clone_7ad990": 0x1407AD990,
    "slot_reset_title_accept_input_temp_child_clone_7ad9d0": 0x1407AD9D0,
    "slot_reset_title_xr_job_7bb010": 0x1407BB010,
    "slot_reset_title_initmenu_gate_builder_7a6c00": 0x1407A6C00,
    "slot_reset_end_flow_branch_e780": 0x140B0E780,
    "slot_reset_end_flow_branch_e650": 0x140B0E650,
    "slot_reset_end_flow_branch_gate_table_init_0a2e10": 0x1400A2E10,
    "slot_reset_end_flow_branch_gate_table_global_43d6f9d0": 0x143D6F9D0,
    "slot_reset_end_flow_branch_gate_countdown_clear_ae5cd0": 0x140AE5CD0,
    "slot_reset_end_flow_branch_gate_countdown_label_42b5a330": 0x142B5A330,
    "slot_reset_end_flow_branch_gate_builder_label_42b5a3d0": 0x142B5A3D0,
    "slot_reset_end_flow_branch_gate_tiny_clear_ae6350": 0x140AE6350,
    "slot_reset_end_flow_branch_gate_tiny_set_ae6360": 0x140AE6360,
    "slot_reset_end_flow_branch_gate_countdown_done_ae5d50": 0x140AE5D50,
    "slot_reset_end_flow_branch_gate_wait_handler_ae5d10": 0x140AE5D10,
    "slot_reset_end_flow_branch_gate_wait_probe_80d5c0": 0x14080D5C0,
    "slot_reset_end_flow_branch_gate_menu_job_wait_ae5390": 0x140AE5390,
    "slot_reset_end_flow_branch_gate_finish_ae5380": 0x140AE5380,
    "slot_reset_branch_gate_resource_75fbd0": 0x14075FBD0,
    "slot_reset_branch_gate_timed_job_7b73d0": 0x1407B73D0,
    "slot_reset_branch_gate_chain_start_ae5180": 0x140AE5180,
    "slot_reset_branch_gate_chain_compose_78e0e0": 0x14078E0E0,
    "slot_reset_branch_gate_chain_step_7927d0": 0x1407927D0,
    "slot_reset_branch_gate_resource_762d50": 0x140762D50,
    "slot_reset_branch_gate_job_7bae20": 0x1407BAE20,
    "slot_reset_branch_gate_condition_ae5230": 0x140AE5230,
    "slot_reset_branch_gate_chain_step_7928a0": 0x1407928A0,
    "slot_reset_branch_gate_submit_ae6520": 0x140AE6520,
    "slot_reset_branch_gate_submit_attach_7a9460": 0x1407A9460,
    "slot_reset_branch_gate_submit_stage_ae5e60": 0x140AE5E60,
    "slot_reset_tail_counter_source_d298c0": 0x140D298C0,
    "slot_reset_tail_calc_bc_29a950": 0x14029A950,
    "slot_reset_tail_mode_start_255930": 0x140255930,
    "slot_reset_tail_mode_cleanup_2609a0": 0x1402609A0,
    "slot_reset_tail_mode_set_5d18f0": 0x1405D18F0,
    "slot_reset_tail_mode_finish_6e9250": 0x1406E9250,
    "slot_reset_tail_game_man_counter_reset_67a730": 0x14067A730,
    "slot_reset_tail_selected_value_set_67ac60": 0x14067AC60,
    "slot_reset_tail_after_selected_value_67aac0": 0x14067AAC0,
    "slot_reset_tail_post_cleanup_1ca1c0": 0x1401CA1C0,
    "slot_reset_tail_calc_bc_720260": 0x140720260,
    "slot_reset_play_game_render_start_e5f8f0": 0x140E5F8F0,
    "slot_reset_play_game_render_finish_e5f7f0": 0x140E5F7F0,
    "slot_reset_play_game_save_slot_get_678ca0": 0x140678CA0,
    "slot_reset_selected_value_b5f_getter_67a070": 0x14067A070,
    "slot_reset_selected_value_snapshot_679450": 0x140679450,
    "slot_reset_selected_value_fallback_679280": 0x140679280,
    "slot_reset_selected_value_mode_check_67a1e0": 0x14067A1E0,
    "slot_reset_selected_value_mode_value_679720": 0x140679720,
    "slot_reset_selected_value_set_save_slot_wrapper_67a820": 0x14067A820,
    "slot_reset_play_game_global_obj_256360": 0x140256360,
    "slot_reset_play_game_prepare_aea590": 0x140AEA590,
    "slot_reset_play_game_submit_aebdc0": 0x140AEBDC0,
    "slot_reset_play_game_submit_selected_value_validate_67aec0": 0x14067AEC0,
    "slot_reset_play_game_submit_validate_prepare_67f300": 0x14067F300,
    "slot_reset_play_game_submit_validate_probe_67d260": 0x14067D260,
    "slot_reset_play_game_submit_load_pair_67abd0": 0x14067ABD0,
    "slot_reset_play_game_submit_load_pair_convert_6783d0": 0x1406783D0,
    "slot_reset_play_game_submit_check_720210": 0x140720210,
    "slot_reset_play_game_submit_copy_aea780": 0x140AEA780,
    "slot_reset_play_game_submit_vector_grow_218c70": 0x140218C70,
    "slot_reset_play_game_consume_owner300_ca89e0": 0x140CA89E0,
    "slot_reset_play_game_probe_a4cf70": 0x140A4CF70,
    "slot_reset_play_game_probe_a4cf90": 0x140A4CF90,
    "slot_reset_play_game_action_a4d7d0": 0x140A4D7D0,
    "slot_reset_play_game_status_256590": 0x140256590,
    "slot_reset_play_game_notify_cc7930": 0x140CC7930,
    "slot_reset_finish_gate_global_43d856a0": 0x143D856A0,
    "slot_reset_set_state_helper_b0d960": 0x140B0D960,
    "slot_reset_menu_job_wait_task_submit_733f20": 0x140733F20,
    "slot_reset_menu_job_wait_queue_7a9600": 0x1407A9600,
    "slot_reset_menu_job_wait_queue_check_7a9200": 0x1407A9200,
    "slot_reset_menu_job_wait_global_toggle_7663c0": 0x1407663C0,
    "slot_reset_timed_descriptor_base_vtable_29c8e48": 0x1429C8E48,
    "slot_reset_timed_descriptor_active_vtable_29c8e58": 0x1429C8E58,
    "slot_reset_global_job_context_43d6b7b0": 0x143D6B7B0,
    "slot_reset_global_context_plus19_gate_758050": 0x140758050,
    "slot_reset_global_context_plus19_gate_status_b3d310": 0x140B3D310,
    "slot_reset_global_toggle_extra_parent_b01be0": 0x140B01BE0,
    "slot_reset_global_toggle_extra_gate_a9cd00": 0x140A9CD00,
    "slot_reset_global_toggle_extra_toggle_gate_a9cc90": 0x140A9CC90,
    "slot_reset_global_toggle_extra_item_a9cdb0": 0x140A9CDB0,
    "slot_reset_global_toggle_extra_status_a9c9d0": 0x140A9C9D0,
    "slot_reset_global_toggle_extra_status_a9cd30": 0x140A9CD30,
    "slot_reset_global_toggle_extra_probe_e2a5c0": 0x140E2A5C0,
    "slot_reset_global_toggle_extra_probe_e2a5e0": 0x140E2A5E0,
    "slot_reset_global_toggle_extra_probe_e29bc0": 0x140E29BC0,
    "slot_reset_global_toggle_extra_probe_e29930": 0x140E29930,
    "slot_reset_global_toggle_extra_callback_7edf40": 0x1407EDF40,
    "slot_reset_global_toggle_extra_callback_7edf90": 0x1407EDF90,
    "slot_reset_handler_b0cd70": 0x140B0CD70,
    "request_save_67a520": 0x14067A520,
    "save_request_profile_67a420": 0x14067A420,
    "request_save_and_profile_gate_67a3a0": 0x14067A3A0,
    "bc4_value_accessor_678f20": 0x140678F20,
    "bc4_is_three_accessor_679f30": 0x140679F30,
    "set_bc4_67a970": 0x14067A970,
    "promote_bc4_2_to_3_67a980": 0x14067A980,
    "post_pump_case2_notify_810970": 0x140810970,
    "task_event_80dc10": 0x14080DC10,
    "task_local_wrapper_7449e0": 0x1407449E0,
    "slot_reset_branch_gate_descriptor_wrapper_744a60": 0x140744A60,
    "title_accept_aux_builder_ac620": 0x1409AC620,
    "title_accept_primary_builder_a6c70": 0x1409A6C70,
    "title_accept_owner_payload_builder_833880": 0x140833880,
    "title_accept_final_owner_builder_9aa2d0": 0x1409AA2D0,
    "title_accept_final_combiner_7ab170": 0x1407AB170,
    "title_accept_final_combiner_inner_7abb40": 0x1407ABB40,
    "title_accept_final_combiner_append_7ac0b0": 0x1407AC0B0,
    "title_accept_attach_7418d0": 0x1407418D0,
    "title_accept_final_wrapper_9aa430": 0x1409AA430,
    "title_accept_final_wrapper_inner_78c530": 0x14078C530,
    "title_accept_branch_compose_inner_7926e0": 0x1407926E0,
    "title_accept_branch_step_swap_7925d0": 0x1407925D0,
    "title_accept_branch_step_builder_792970": 0x140792970,
    "title_accept_branch_step_build_inner_792100": 0x140792100,
    "title_accept_branch_step_payload_791b50": 0x140791B50,
    "title_accept_branch_step_payload_vtable_aa2938": 0x142AA2938,
    "title_accept_branch_step_status_vtable_aa2958": 0x142AA2958,
    "title_accept_branch_step_status_vslot2_7aa1f0": 0x1407AA1F0,
    "title_accept_branch_step_payload_vslot2_792460": 0x140792460,
    "descriptor_status_gt_one_7a9200": 0x1407A9200,
    "descriptor_status_eq_two_7a9210": 0x1407A9210,
    "title_accept_branch_step_condition_init_7923f0": 0x1407923F0,
    "title_accept_branch_step_condition_add_793770": 0x140793770,
    "title_accept_branch_step_condition_finish_7936f0": 0x1407936F0,
    "title_accept_branch_step_condition_cleanup_791ed0": 0x140791ED0,
    "title_accept_branch_step_attach_7aa380": 0x1407AA380,
    "title_accept_final_cleanup_78cec0": 0x14078CEC0,
    "task_alloc_selector_high_7a7200": 0x1407A7200,
    "task_alloc_selector_low_7a7250": 0x1407A7250,
    "title_accept_primary_chain_builder_7a72b0": 0x1407A72B0,
    "task_enqueue_7a7b60": 0x1407A7B60,
    "task_enqueue_link_7a7bb0": 0x1407A7BB0,
    "selector_builder_local_wrapper_744d10": 0x140744D10,
    "selector_builder_input_key_7600a0": 0x1407600A0,
    "selector_builder_context_init_78c950": 0x14078C950,
    "selector_builder_chain_key_7a91e0": 0x1407A91E0,
    "menu_task_state_clear_7a91f0": 0x1407A91F0,
    "menu_task_state_compare_7a9200": 0x1407A9200,
    "menu_task_state_is_two_7a9210": 0x1407A9210,
    "menu_task_state_payload_ptr_7a9220": 0x1407A9220,
    "menu_task_state_is_empty_7a9230": 0x1407A9230,
    "menu_state_delay_consumer_ctor_81dfa0": 0x14081DFA0,
    "menu_state_bridge_consumer_ctor_823d30": 0x140823D30,
    "menu_state_payload_consumer_ctor_82efe0": 0x14082EFE0,
    "selector_builder_chain_append_7ccbb0": 0x1407CCBB0,
    "selector_builder_chain_submit_78dac0": 0x14078DAC0,
    "selector_builder_context_cleanup_743700": 0x140743700,
    "selector6_builder_context_827bd0": 0x140827BD0,
    "selector6_builder_direct_caller_825f70": 0x140825F70,
    "selector6_builder_parent_thunk_822460": 0x140822460,
    "selector6_builder_outer_thunk_823950": 0x140823950,
    "selector6_builder_entry_thunk_822c70": 0x140822C70,
    "selector_builder_fallback_compose_828570": 0x140828570,
    "entry_selector_824b50": 0x140824B50,
    "selector2_entry_func_8247e0": 0x1408247E0,
    "selector7_entry_func_824960": 0x140824960,
    "selector1_entry_func_824b10": 0x140824B10,
    "selector6_entry_func_8257a0": 0x1408257A0,
    "selector3_entry_func_825910": 0x140825910,
    "selector4_entry_func_825e00": 0x140825E00,
    "selector6_parent_thunk_8222b0": 0x1408222B0,
    "selector3_parent_thunk_822340": 0x140822340,
    "selector4_parent_thunk_8223d0": 0x1408223D0,
    "selector6_outer_thunk_8237d0": 0x1408237D0,
    "selector3_outer_thunk_823850": 0x140823850,
    "selector4_outer_thunk_8238d0": 0x1408238D0,
    "selector6_entry_level_822af0": 0x140822AF0,
    "selector3_entry_level_822b70": 0x140822B70,
    "selector4_entry_level_822bf0": 0x140822BF0,
    "selector6_entry_helper_82a1f0": 0x14082A1F0,
    "selector3_entry_helper_82a2c0": 0x14082A2C0,
    "selector4_entry_helper_82a360": 0x14082A360,
    "selector6_builder_entry_helper_82a400": 0x14082A400,
    "selector6_builder_entry_ctor_thunk_827770": 0x140827770,
    "selector6_builder_entry_alloc_clone_822680": 0x140822680,
    "selector6_builder_entry_copy_ctor_8233c0": 0x1408233C0,
    "selector6_builder_entry_clone_plus8_823990": 0x140823990,
    "selector6_builder_entry_copy_wrapper_822f60": 0x140822F60,
    "selector6_builder_entry_owner_init_821b70": 0x140821B70,
    "selector6_builder_entry_owner_compose_828830": 0x140828830,
    "selector6_builder_entry_owner_compose_parent_8288e0": 0x1408288E0,
    "selector6_owner_variant_local_828cb0": 0x140828CB0,
    "selector6_owner_variant_local_828e10": 0x140828E10,
    "selector6_owner_variant_input_82a970": 0x14082A970,
    "selector6_owner_variant_wrapper_828450": 0x140828450,
    "selector6_owner_preflight_slot_826ed0": 0x140826ED0,
    "selector_owner_ctor_821e00": 0x140821E00,
    "selector_owner_dtor_824220": 0x140824220,
    "selector_owner_delete_wrapper_826180": 0x140826180,
    "selector_owner_ctor_wrapper_8263c0": 0x1408263C0,
    "selector_owner_sibling_ctor_wrapper_826630": 0x140826630,
    "selector_owner_factory_830210": 0x140830210,
    "selector_owner_factory_thunk_82dcb0": 0x14082DCB0,
    "selector_owner_factory_outer_82ec20": 0x14082EC20,
    "selector_owner_factory_outer_caller_82e350": 0x14082E350,
    "selector_owner_factory_entry_helper_837070": 0x140837070,
    "selector_owner_factory_neighbor_entry_helper_837020": 0x140837020,
    "selector_owner_factory_neighbor_outer_caller_82e310": 0x14082E310,
    "selector_owner_factory_neighbor_outer_82ebe0": 0x14082EBE0,
    "selector_owner_factory_neighbor_thunk_82dc70": 0x14082DC70,
    "selector_owner_factory_neighbor_factory_82fda0": 0x14082FDA0,
    "selector_owner_factory_entry_copy_835380": 0x140835380,
    "selector_owner_factory_entry_dtor_8363a0": 0x1408363A0,
    "selector_owner_factory_entry_clone_8380c0": 0x1408380C0,
    "selector_factory_entry_complex_builder_8394b0": 0x1408394B0,
    "selector_factory_entry_complex_submit_834b40": 0x140834B40,
    "selector_submit_descriptor_builder_82d840": 0x14082D840,
    "selector_submit_clone_wrapper_8347a0": 0x1408347A0,
    "selector_submit_final_enqueue_7917e0": 0x1407917E0,
    "selector_final_enqueue_pair_builder_790fa0": 0x140790FA0,
    "selector6_builder_entry_owner_cleanup_823fe0": 0x140823FE0,
    "selector_owner_parent_input_guard_765030": 0x140765030,
    "pump_owner_builder_828fd0": 0x140828FD0,
    "entry_family_builder_8279e0": 0x1408279E0,
    "positive_delay_builder_caller_8249a0": 0x1408249A0,
    "other_load_builder_caller_824a80": 0x140824A80,
    "new_or_load_builder_caller_8250e0": 0x1408250E0,
    "continue_builder_caller_8257e0": 0x1408257E0,
    "positive_delay_thunk_821fe0": 0x140821FE0,
    "other_load_thunk_822020": 0x140822020,
    "new_or_load_thunk_8221b0": 0x1408221B0,
    "continue_thunk_822300": 0x140822300,
    "positive_delay_outer_823510": 0x140823510,
    "other_load_outer_823550": 0x140823550,
    "new_or_load_outer_8236d0": 0x1408236D0,
    "continue_outer_823810": 0x140823810,
    "positive_delay_entry_8227d0": 0x1408227D0,
    "other_load_entry_822810": 0x140822810,
    "new_or_load_entry_8229f0": 0x1408229F0,
    "continue_entry_822b30": 0x140822B30,
    "positive_entry_helper_829d40": 0x140829D40,
    "other_load_entry_helper_829d90": 0x140829D90,
    "new_or_load_entry_helper_82a000": 0x14082A000,
    "continue_entry_helper_82a270": 0x14082A270,
    "positive_entry_ctor_827250": 0x140827250,
    "other_load_entry_ctor_827290": 0x140827290,
    "new_or_load_entry_ctor_8274a0": 0x1408274A0,
    "continue_entry_ctor_827660": 0x140827660,
    "positive_entry_copy_82aff0": 0x14082AFF0,
    "other_load_entry_copy_82b030": 0x14082B030,
    "new_or_load_entry_copy_82b2b0": 0x14082B2B0,
    "continue_entry_copy_82b590": 0x14082B590,
    "combined_load_67b940": 0x14067B940,
    "continue_load_67b750": 0x14067B750,
    "current_slot_load_67b570": 0x14067B570,
    "slot_reset_title_accept_input_manager_gate_766010": 0x140766010,
    "title_top_dialog_pulse1_candidate_9b26d8": 0x1409B26D8,
}

GHIDRA_RECONCILIATION_TARGETS = [
    {"name": "slot_reset_title_begintitle_bool_wrapper_b0c180", "expects_function_entry": True},
    {"name": "slot_reset_title_begintitle_owner138_e0_builder_81f9f0", "expects_function_entry": False},
    {"name": "slot_reset_title_accept_input_condition_builder_7acb00", "expects_function_entry": False},
    {"name": "slot_reset_title_accept_input_node_update_7ad1c0", "expects_function_entry": False},
    {"name": "slot_reset_title_accept_input_manager_state_765f20", "expects_function_entry": True},
    {"name": "slot_reset_title_accept_input_manager_gate_766010", "expects_function_entry": True},
    {"name": "slot_reset_title_accept_input_manager_queue_setup_7660f0", "expects_function_entry": True},
    {"name": "slot_reset_to_menu_job_wait_helper_b0e530", "expects_function_entry": False},
    {"name": "slot_reset_menu_job_wait_handler_b0d400", "expects_function_entry": True},
    {"name": "title_top_dialog_pulse1_candidate_9b26d8", "expects_function_entry": False},
]

FIELD_OFFSETS = {
    "b72_save_request_profile": 0xB72,
    "b73_save_request": 0xB73,
    "b5e_end_flow_flag": 0xB5E,
    "b75_load_arg": 0xB75,
    "b78_requested_slot_index": 0xB78,
    "b80_load_state": 0xB80,
    "bb8_pending_transition": 0xBB8,
    "bbc_transition_state": 0xBBC,
    "bc0_transition_count": 0xBC0,
    "bc4_queue_gate": 0xBC4,
}

FIELD_ACCESSOR_WINDOWS = {
    "b72_profile_flag_accessor_6793d0": (0x1406793D0, 0x58),
    "b73_request_flag_accessor_679370": (0x140679370, 0x44),
    "b75_load_arg_accessor_679100": (0x140679100, 0x10),
    "b78_requested_slot_accessor_6793c0": (0x1406793C0, 0x10),
    "pump_delta_6794b0": (0x1406794B0, 0x52),
    "pump_default_679510": (0x140679510, 0x4A),
    "save_request_gate_67a3a0": (0x14067A3A0, 0x78),
    "b5e_bulk_clear_679830": (0x140679830, 0x43),
    "b5e_getter_67a1b0": (0x14067A1B0, 0x0F),
    "b5e_setter_67ae90": (0x14067AE90, 0x0E),
    "b5e_reset_clear_67e21d": (0x14067E21D, 0x64),
    "bc4_value_accessor_678f20": (0x140678F20, 0x0E),
    "bc4_is_three_accessor_679f30": (0x140679F30, 0x12),
    "set_bc4_67a970": (0x14067A970, 0x0E),
    "promote_bc4_2_to_3_67a980": (0x14067A980, 0x1B),
}

JUMP_TABLES = {
    # 0x140afbae6 calls save_load_pump_default_679510 and dispatches
    # return values 0..9 through imagebase-relative RVAs at 0x140afbd04.
    "post_pump_default_switch_afbd04": (0x140AFBD04, 10),
}

VTABLE_GROUP_BASES = {
    "menu_task_family_7220": 0x142AC7220,
    "menu_task_family_7258": 0x142AC7258,
    "pump_owner_vtable_7290": 0x142AC7290,
    "case8_related_vtable_72c8": 0x142AC72C8,
    "save_select_related_vtable_7300": 0x142AC7300,
}

SWITCH_TARGET_WINDOWS = {
    "case0_promote_bc4": (0x140AFBB17, 0x39),
    "case1_completion_flag": (0x140AFBB71, 0x55),
    "case2_event_reset": (0x140AFBB5B, 0x12),
    "case8_context_path": (0x140AFBBD5, 0x40),
}

TASK_OBJECT_WINDOWS = {
    "pump_owner_ctor_827530": (0x140827530, 0x40),
    "pump_owner_dtor_829840": (0x140829840, 0x38),
    "pump_owner_clone_82b3a0": (0x14082B3A0, 0x42),
    "pump_owner_update_82a0f0": (0x14082A0F0, 0x4D),
    "pump_owner_local_builder_829000": (0x140829000, 0x82),
}

BUILDER_CALLSITE_WINDOWS = {
    "positive_delay_ba30": (0x1408249DC, 0x42),
    "other_load_bb00_zero": (0x140824ABC, 0x38),
    "new_or_load_ba80_zero": (0x140825120, 0x34),
    "continue_bac0_zero": (0x140825820, 0x34),
}

FUNCTION_RANGE_PROBES = {
    "positive_delay_builder_caller": 0x140824A14,
    "other_load_builder_caller": 0x140824AEF,
    "new_or_load_builder_caller": 0x14082514F,
    "continue_builder_caller": 0x14082584F,
    "pump_owner_builder": 0x140828FD0,
    "positive_delay_wrapper_ba30": 0x14082BA30,
    "new_or_load_wrapper_ba80": 0x14082BA80,
    "continue_wrapper_bac0": 0x14082BAC0,
    "other_load_wrapper_bb00": 0x14082BB00,
}

BUILDER_CALLER_SITES = {
    "positive_delay_builder_caller": 0x140824A14,
    "other_load_builder_caller": 0x140824AEF,
    "new_or_load_builder_caller": 0x14082514F,
    "continue_builder_caller": 0x14082584F,
}

BUILDER_CONSTRUCTOR_TARGETS = {
    "positive_delay_builder_caller_8249a0": 0x1408249A0,
    "other_load_builder_caller_824a80": 0x140824A80,
    "new_or_load_builder_caller_8250e0": 0x1408250E0,
    "continue_builder_caller_8257e0": 0x1408257E0,
}

WRAPPER_THUNK_TARGETS = {
    "positive_delay_thunk_821fe0": 0x140821FE0,
    "other_load_thunk_822020": 0x140822020,
    "new_or_load_thunk_8221b0": 0x1408221B0,
    "continue_thunk_822300": 0x140822300,
}

OUTER_THUNK_TARGETS = {
    "positive_delay_outer_823510": 0x140823510,
    "other_load_outer_823550": 0x140823550,
    "new_or_load_outer_8236d0": 0x1408236D0,
    "continue_outer_823810": 0x140823810,
}

ENTRY_THUNK_TARGETS = {
    "positive_delay_entry_8227d0": 0x1408227D0,
    "other_load_entry_822810": 0x140822810,
    "new_or_load_entry_8229f0": 0x1408229F0,
    "continue_entry_822b30": 0x140822B30,
}

ENTRY_HELPER_TARGETS = {
    "positive_entry_helper_829d40": 0x140829D40,
    "other_load_entry_helper_829d90": 0x140829D90,
    "new_or_load_entry_helper_82a000": 0x14082A000,
    "continue_entry_helper_82a270": 0x14082A270,
}

ENTRY_HELPER_VTABLE_BASES = {
    "positive_entry_vtable_7370": 0x142AC7370,
    "continue_entry_vtable_7878": 0x142AC7878,
    "new_or_load_entry_vtable_78e8": 0x142AC78E8,
    "other_load_entry_vtable_7920": 0x142AC7920,
}

ENTRY_TASK_CTOR_COPY_TARGET_NAMES = [
    "positive_entry_ctor_827250",
    "other_load_entry_ctor_827290",
    "new_or_load_entry_ctor_8274a0",
    "continue_entry_ctor_827660",
    "positive_entry_copy_82aff0",
    "other_load_entry_copy_82b030",
    "new_or_load_entry_copy_82b2b0",
    "continue_entry_copy_82b590",
]

ENTRY_FAMILY_DESCRIPTOR_VTABLE_BASES = {
    "entry_family_descriptor_vtable_73a8": 0x142AC73A8,
}

ENTRY_SELECTOR_TARGET_NAMES = [
    "selector2_entry_func_8247e0",
    "selector7_entry_func_824960",
    "selector1_entry_func_824b10",
    "selector6_entry_func_8257a0",
    "selector3_entry_func_825910",
    "selector4_entry_func_825e00",
]

ENTRY_SELECTOR_PARENT_TARGET_NAMES = [
    "selector6_parent_thunk_8222b0",
    "selector3_parent_thunk_822340",
    "selector4_parent_thunk_8223d0",
]

ENTRY_SELECTOR_OUTER_TARGET_NAMES = [
    "selector6_outer_thunk_8237d0",
    "selector3_outer_thunk_823850",
    "selector4_outer_thunk_8238d0",
]

ENTRY_LEVEL_TARGET_NAMES = [
    "selector6_entry_level_822af0",
    "continue_entry_822b30",
    "selector3_entry_level_822b70",
    "selector4_entry_level_822bf0",
    "selector6_builder_entry_thunk_822c70",
]

SELECTOR_ENTRY_HELPER_TARGET_NAMES = [
    "selector6_entry_helper_82a1f0",
    "selector3_entry_helper_82a2c0",
    "selector4_entry_helper_82a360",
    "selector6_builder_entry_helper_82a400",
]

SELECTOR_ENTRY_VTABLE_BASES = {
    "selector6_entry_vtable_75a0": 0x142AC75A0,
    "selector3_entry_vtable_74f8": 0x142AC74F8,
    "selector4_entry_vtable_7530": 0x142AC7530,
    "selector6_builder_entry_vtable_7648": 0x142AC7648,
}

SELECTOR_BUILDER_CONTEXT = (0x140827BD0, 0x14082826F)
SELECTOR_BUILDER_DIRECT_CALLER = (0x140825F70, 0x1408260AC)
SELECTOR_BUILDER_GLOBAL_FAST_PATH_FLAG = 0x143D6CD80

SELECTOR_BUILDER_DESCRIPTOR_VTABLES = {
    "selector_unknown_7488": 0x142AC7488,
    "selector_unknown_74c0": 0x142AC74C0,
    "selector3_entry_vtable_74f8": 0x142AC74F8,
    "selector4_entry_vtable_7530": 0x142AC7530,
    "selector_unknown_7568": 0x142AC7568,
    "selector6_entry_vtable_75a0": 0x142AC75A0,
}

SELECTOR_BUILDER_WRAPPER_TAGS = {
    "tag_70f8": 0x142AC70F8,
    "tag_70e8": 0x142AC70E8,
    "tag_70d0": 0x142AC70D0,
    "tag_70c0": 0x142AC70C0,
    "tag_a86538": 0x142A86538,
}

SELECTOR_OWNER_PARENT_TARGETS = {
    "owner_parent_common_vtable_7188": 0x142AC7188,
    "owner_parent_common_base_142a93c38": 0x142A93C38,
    "owner_parent_local_tag_7120": 0x142AC7120,
    "owner_parent_local_tag_7130": 0x142AC7130,
    "owner_parent_input_tag_7108": 0x142AC7108,
    "owner_parent_dispatch_vtable_7728": 0x142AC7728,
    "owner_parent_dispatch_vtable_7760": 0x142AC7760,
    "owner_parent_input_vtable_76f0": 0x142AC76F0,
    "owner_parent_callback_a750": 0x14082A750,
    "owner_parent_callback_ab20": 0x14082AB20,
    "owner_parent_callback_c240": 0x14082C240,
}

SELECTOR_OWNER_VARIANT_TARGET_NAMES = [
    "selector6_owner_variant_local_828cb0",
    "selector6_owner_variant_local_828e10",
    "selector6_owner_variant_input_82a970",
]

SELECTOR_OWNER_VARIANT_LOCAL_DESCRIPTOR_VTABLES = {
    "owner_variant_continue_neighbor_7840": 0x142AC7840,
    "owner_variant_local_7808": 0x142AC7808,
    "owner_variant_local_7798": 0x142AC7798,
}

CONTINUE_SELECTOR_COMPARISON_VTABLE_SLOTS = {
    "continue_entry_helper_slot_7888": 0x142AC7888,
    "selector6_builder_entry_helper_slot_7658": 0x142AC7658,
    "selector6_owner_preflight_slot_71d0": 0x142AC71D0,
}

SELECTOR_OWNER_LIFECYCLE_VTABLE_BASES = {
    "selector_owner_preflight_vtable_71c0": 0x142AC71C0,
    "selector_owner_sibling_vtable_71e0": 0x142AC71E0,
    "selector_owner_sibling_vtable_7200": 0x142AC7200,
}

SELECTOR_FACTORY_ENTRY_VTABLES = {
    "selector_factory_entry_vtable_b5c8": 0x142ACB5C8,
    "selector_factory_entry_neighbor_vtable_bcc8": 0x142ACBCC8,
}

SELECTOR_SUBMIT_DESCRIPTOR_VTABLES = {
    "selector_submit_descriptor_vtable_bde0": 0x142ACBDE0,
}


def parse_pe(data: bytes) -> tuple[int, list[dict[str, int | str]]]:
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    section_count = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    image_base_offset = optional + 24
    image_base = struct.unpack_from("<Q" if magic == 0x20B else "<I", data, image_base_offset)[0]
    section_offset = optional + optional_size
    sections: list[dict[str, int | str]] = []
    for index in range(section_count):
        offset = section_offset + index * 40
        name = data[offset : offset + 8].split(b"\0", 1)[0].decode("ascii", errors="replace")
        virtual_size, virtual_address, raw_size, raw_ptr = struct.unpack_from("<IIII", data, offset + 8)
        sections.append(
            {
                "name": name,
                "virtual_address": virtual_address,
                "virtual_size": max(virtual_size, raw_size),
                "raw_size": raw_size,
                "raw_ptr": raw_ptr,
            }
        )
    return image_base, sections


def section_for_va(image_base: int, sections: list[dict[str, int | str]], va: int) -> dict[str, int | str] | None:
    rva = va - image_base
    for section in sections:
        start = int(section["virtual_address"])
        end = start + int(section["virtual_size"])
        if start <= rva < end:
            return section
    return None


def va_to_file_offset(image_base: int, sections: list[dict[str, int | str]], va: int) -> int | None:
    section = section_for_va(image_base, sections, va)
    if section is None:
        return None
    rva = va - image_base
    return int(section["raw_ptr"]) + (rva - int(section["virtual_address"]))


def offset_to_va(image_base: int, sections: list[dict[str, int | str]], offset: int) -> tuple[int, str] | None:
    for section in sections:
        raw_ptr = int(section["raw_ptr"])
        raw_size = int(section["raw_size"])
        if raw_ptr <= offset < raw_ptr + raw_size:
            return image_base + int(section["virtual_address"]) + (offset - raw_ptr), str(section["name"])
    return None


def scan_rel32_calls(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, list[dict[str, Any]]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    by_target: dict[int, list[str]] = {}
    for name, addr in TARGETS.items():
        by_target.setdefault(addr, []).append(name)
    refs: dict[str, list[dict[str, Any]]] = {name: [] for name in TARGETS}
    for index in range(0, max(0, len(text_data) - 5)):
        opcode = text_data[index]
        if opcode not in (0xE8, 0xE9):
            continue
        rel = struct.unpack_from("<i", text_data, index + 1)[0]
        src = text_va + index
        target = src + 5 + rel
        target_names = by_target.get(target)
        if target_names is None:
            continue
        ref = {
            "kind": "call" if opcode == 0xE8 else "jmp",
            "source_va": f"0x{src:x}",
            "target_va": f"0x{target:x}",
            "source_rva": f"0x{src - image_base:x}",
            "target_rva": f"0x{target - image_base:x}",
        }
        for target_name in target_names:
            refs[target_name].append(dict(ref))
    return refs


def scan_absolute_qword_refs(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, list[dict[str, Any]]]:
    refs: dict[str, list[dict[str, Any]]] = {name: [] for name in TARGETS}
    for name, target in TARGETS.items():
        needle = struct.pack("<Q", target)
        start = 0
        while True:
            index = data.find(needle, start)
            if index < 0:
                break
            mapped = offset_to_va(image_base, sections, index)
            if mapped is not None:
                va, section = mapped
                refs[name].append(
                    {
                        "section": section,
                        "source_va": f"0x{va:x}",
                        "source_rva": f"0x{va - image_base:x}",
                        "target_va": f"0x{target:x}",
                        "target_rva": f"0x{target - image_base:x}",
                    }
                )
            start = index + 1
    return refs


def read_qwords(data: bytes, image_base: int, sections: list[dict[str, int | str]], start_va: int, count: int) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for offset in range(count):
        va = start_va + offset * 8
        file_offset = va_to_file_offset(image_base, sections, va)
        if file_offset is None or file_offset + 8 > len(data):
            continue
        value = struct.unpack_from("<Q", data, file_offset)[0]
        rows.append({"entry_va": f"0x{va:x}", "value_va": f"0x{value:x}", "value_rva": f"0x{value - image_base:x}"})
    return rows


def read_qword_value(data: bytes, image_base: int, sections: list[dict[str, int | str]], va: int) -> int | None:
    file_offset = va_to_file_offset(image_base, sections, va)
    if file_offset is None or file_offset + 8 > len(data):
        return None
    return struct.unpack_from("<Q", data, file_offset)[0]


def function_has_rip_lea_to(
    data: bytes, image_base: int, sections: list[dict[str, int | str]], start_va: int | None, target_va: int, size: int = 0x60
) -> bool:
    if start_va is None:
        return False
    blob = read_bytes(data, image_base, sections, start_va, size)
    for index in range(max(0, len(blob) - 7)):
        if blob[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
            continue
        disp = struct.unpack_from("<i", blob, index + 3)[0]
        if start_va + index + 7 + disp == target_va:
            return True
    return False


def function_has_rip_lea_any_reg_to(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    start_va: int | None,
    target_va: int,
    size: int = 0x60,
) -> bool:
    if start_va is None:
        return False
    blob = read_bytes(data, image_base, sections, start_va, size)
    for index in range(max(0, len(blob) - 7)):
        if blob[index] not in (0x48, 0x4C) or blob[index + 1] != 0x8D:
            continue
        # RIP-relative LEA has mod=00 and r/m=101 in the ModRM byte; the reg
        # field selects the destination register and may vary (RAX/RCX/RBP/RDI).
        if blob[index + 2] & 0xC7 != 0x05:
            continue
        disp = struct.unpack_from("<i", blob, index + 3)[0]
        if start_va + index + 7 + disp == target_va:
            return True
    return False


def function_tail_jump_target(
    data: bytes, image_base: int, sections: list[dict[str, int | str]], start_va: int | None, size: int = 0x40
) -> int | None:
    if start_va is None:
        return None
    blob = read_bytes(data, image_base, sections, start_va, size)
    for index in range(max(0, len(blob) - 5)):
        if blob[index] != 0xE9:
            continue
        return start_va + index + 5 + struct.unpack_from("<i", blob, index + 1)[0]
    return None


def read_bytes(data: bytes, image_base: int, sections: list[dict[str, int | str]], start_va: int, size: int) -> bytes:
    file_offset = va_to_file_offset(image_base, sections, start_va)
    if file_offset is None:
        return b""
    return data[file_offset : min(len(data), file_offset + size)]


def scan_field_windows(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, dict[str, Any]]:
    windows: dict[str, dict[str, Any]] = {}
    for name, (start_va, size) in FIELD_ACCESSOR_WINDOWS.items():
        blob = read_bytes(data, image_base, sections, start_va, size)
        offsets: list[dict[str, str]] = []
        for field_name, field_offset in FIELD_OFFSETS.items():
            needle = struct.pack("<I", field_offset)
            cursor = 0
            while True:
                index = blob.find(needle, cursor)
                if index < 0:
                    break
                offsets.append(
                    {
                        "field": field_name,
                        "offset": f"0x{field_offset:x}",
                        "at_va": f"0x{start_va + index:x}",
                    }
                )
                cursor = index + 1
        windows[name] = {
            "start_va": f"0x{start_va:x}",
            "start_rva": f"0x{start_va - image_base:x}",
            "size": size,
            "field_offsets_seen": offsets,
            "bytes_hex": blob.hex(),
        }
    return windows


def fields_seen(field_windows: dict[str, dict[str, Any]], window: str) -> set[str]:
    return {entry["field"] for entry in field_windows.get(window, {}).get("field_offsets_seen", [])}


def ref_source_set(refs: dict[str, list[dict[str, Any]]], target_name: str) -> set[str]:
    return {str(ref["source_va"]) for ref in refs.get(target_name, [])}


def _normalized_source_set(rows: list[dict[str, Any]]) -> set[str]:
    return {str(row.get("source_va", "")).lower() for row in rows if row.get("source_va")}


def build_ghidra_reconciliation(
    repo_root: Path,
    image_base: int,
    rel32_refs: dict[str, list[dict[str, Any]]],
    absolute_refs: dict[str, list[dict[str, Any]]],
    exe_md5: str,
) -> dict[str, Any]:
    facts_path = Path(
        os.environ.get("GHIDRA_ADDRESS_FACTS_PATH", str(repo_root / "target/ghidra/ghidra-address-facts.json"))
    )
    if not facts_path.exists():
        return {
            "status": "missing_ghidra_facts",
            "facts_path": str(facts_path),
            "summary": {
                "score": 0,
                "target_count": len(GHIDRA_RECONCILIATION_TARGETS),
                "static_ref_match_count": 0,
                "function_boundary_mismatch_count": 0,
                "program_md5_matches_local": False,
            },
            "targets": [],
        }
    try:
        facts = json.loads(facts_path.read_text(encoding="utf-8", errors="replace"))
    except Exception as error:
        return {
            "status": "invalid_ghidra_facts",
            "facts_path": str(facts_path),
            "error": str(error),
            "summary": {
                "score": 0,
                "target_count": len(GHIDRA_RECONCILIATION_TARGETS),
                "static_ref_match_count": 0,
                "function_boundary_mismatch_count": 0,
                "program_md5_matches_local": False,
            },
            "targets": [],
        }

    program = facts.get("program", {}) if isinstance(facts, dict) else {}
    ghidra_md5 = str(program.get("executable_md5") or "").lower()
    program_md5_matches = bool(ghidra_md5 and ghidra_md5 == exe_md5.lower())
    by_name = {str(row.get("name")): row for row in facts.get("targets", []) if isinstance(row, dict)}
    rows: list[dict[str, Any]] = []
    score = 20 if program_md5_matches else 0
    function_found_count = 0
    user_defined_count = 0
    static_ref_match_count = 0
    function_boundary_mismatch_count = 0
    missing_fact_count = 0

    for target in GHIDRA_RECONCILIATION_TARGETS:
        name = str(target["name"])
        target_va = f"0x{TARGETS[name]:x}"
        fact = by_name.get(name)
        if fact is None:
            missing_fact_count += 1
            rows.append({"name": name, "target_va": target_va, "status": "missing_ghidra_fact"})
            continue

        function = fact.get("function") if isinstance(fact.get("function"), dict) else None
        function_entry = str((function or {}).get("entry_va") or "").lower()
        function_source = str((function or {}).get("source") or "")
        function_found = function is not None
        if function_found:
            function_found_count += 1
        symbols = fact.get("symbols", []) if isinstance(fact.get("symbols"), list) else []
        user_defined = function_source == "USER_DEFINED" or any(
            str(symbol.get("source") or "") == "USER_DEFINED" for symbol in symbols if isinstance(symbol, dict)
        )
        if user_defined:
            user_defined_count += 1
        expects_entry = bool(target.get("expects_function_entry"))
        boundary_matches = function_entry == target_va
        if expects_entry and function_found and not boundary_matches:
            function_boundary_mismatch_count += 1

        local_sources = {
            str(row.get("source_va", "")).lower()
            for row in rel32_refs.get(name, []) + absolute_refs.get(name, [])
            if row.get("source_va")
        }
        ghidra_exact_sources = _normalized_source_set(fact.get("refs_to_exact", []))
        ghidra_entry_sources = _normalized_source_set(fact.get("refs_to_function_entry", []))
        matched_sources = sorted(local_sources & (ghidra_exact_sources | ghidra_entry_sources), key=lambda value: int(value, 16))
        if matched_sources:
            static_ref_match_count += len(matched_sources)

        row_score = 0
        if function_found:
            row_score += 10
        if not expects_entry or boundary_matches:
            row_score += 10
        if matched_sources:
            row_score += 20
        if user_defined:
            row_score += 10
        score += row_score
        rows.append(
            {
                "name": name,
                "target_va": target_va,
                "expects_function_entry": expects_entry,
                "function_name": (function or {}).get("name"),
                "function_entry_va": function_entry or None,
                "function_source": function_source or None,
                "function_boundary_matches_target": boundary_matches,
                "user_defined_symbol_or_function": user_defined,
                "local_static_ref_count": len(local_sources),
                "ghidra_exact_ref_count": len(ghidra_exact_sources),
                "ghidra_function_entry_ref_count": len(ghidra_entry_sources),
                "matched_static_ref_sources": matched_sources,
                "row_score": row_score,
            }
        )

    return {
        "status": "ok" if program_md5_matches else "program_md5_mismatch_or_unknown",
        "facts_path": str(facts_path),
        "program": program,
        "local_image_base": f"0x{image_base:x}",
        "local_exe_md5": exe_md5,
        "summary": {
            "score": score,
            "target_count": len(GHIDRA_RECONCILIATION_TARGETS),
            "function_found_count": function_found_count,
            "user_defined_count": user_defined_count,
            "static_ref_match_count": static_ref_match_count,
            "function_boundary_mismatch_count": function_boundary_mismatch_count,
            "missing_fact_count": missing_fact_count,
            "program_md5_matches_local": program_md5_matches,
        },
        "targets": rows,
    }


def read_jump_tables(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, list[dict[str, Any]]]:
    tables: dict[str, list[dict[str, Any]]] = {}
    for name, (table_va, count) in JUMP_TABLES.items():
        file_offset = va_to_file_offset(image_base, sections, table_va)
        rows: list[dict[str, Any]] = []
        if file_offset is not None:
            for index in range(count):
                entry_offset = file_offset + index * 4
                if entry_offset + 4 > len(data):
                    break
                target_rva = struct.unpack_from("<i", data, entry_offset)[0]
                rows.append(
                    {
                        "case": index,
                        "entry_va": f"0x{table_va + index * 4:x}",
                        "target_rva": f"0x{target_rva & 0xffffffff:x}",
                        "target_va": f"0x{image_base + target_rva:x}",
                    }
                )
        tables[name] = rows
    return tables


def read_switch_target_windows(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, dict[str, Any]]:
    windows: dict[str, dict[str, Any]] = {}
    for name, (start_va, size) in SWITCH_TARGET_WINDOWS.items():
        blob = read_bytes(data, image_base, sections, start_va, size)
        windows[name] = {
            "start_va": f"0x{start_va:x}",
            "start_rva": f"0x{start_va - image_base:x}",
            "size": size,
            "bytes_hex": blob.hex(),
            "checks_task_12a": b"\x80\xbb\x2a\x01\x00\x00\x00" in blob,
            "sets_global_82a8": b"\xc6\x80\xa8\x82\x00\x00\x01" in blob,
            "loads_case8_context_global": b"\x48\x8b\x0d\x6c\xc2\x26\x03" in blob,
        }
    return windows


def read_task_object_windows(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, dict[str, Any]]:
    windows: dict[str, dict[str, Any]] = {}
    for name, (start_va, size) in TASK_OBJECT_WINDOWS.items():
        blob = read_bytes(data, image_base, sections, start_va, size)
        windows[name] = {
            "start_va": f"0x{start_va:x}",
            "start_rva": f"0x{start_va - image_base:x}",
            "size": size,
            "bytes_hex": blob.hex(),
            "loads_task_plus8_float": b"\xf3\x0f\x10\x41\x08" in blob,
            "stores_task_plus8_float": b"\xf3\x0f\x11\x42\x08" in blob,
            "stores_xmm2_to_local_task_plus8": b"\xf3\x0f\x11\x55\xff" in blob,
            "compares_plus8_to_zero": b"\x0f\x2f\xc1" in blob,
            "branches_to_default_on_nonpositive": b"\x76\x07" in blob,
        }
    return windows


def read_menu_load_pump_handoff_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    state_submit_va = TARGETS["selector_builder_chain_key_7a91e0"]
    state_submit_blob = read_bytes(data, image_base, sections, state_submit_va, 10)
    state_submit_refs = rel32_refs.get("selector_builder_chain_key_7a91e0", [])
    state_submit_sources = {str(ref.get("source_va")) for ref in state_submit_refs}
    wrapper_specs = {
        "new_or_current_slot_wrapper": {
            "begin": TARGETS["menu_new_or_load_wrapper"],
            "primitive": "current_slot_load_67b570",
            "primitive_call": "0x14082ba90",
            "submit_call": "0x14082baa7",
        },
        "continue_wrapper": {
            "begin": TARGETS["menu_continue_wrapper"],
            "primitive": "continue_load_67b750",
            "primitive_call": "0x14082bad1",
            "submit_call": "0x14082bae8",
        },
        "other_map_load_wrapper": {
            "begin": TARGETS["menu_other_load_wrapper"],
            "primitive": "map_load_67bc10",
            "primitive_call": "0x14082bb09",
            "submit_call": "0x14082bb20",
        },
    }
    wrappers: dict[str, dict[str, Any]] = {}
    submit_sources = state_submit_sources
    for name, spec in wrapper_specs.items():
        begin = int(spec["begin"])
        range_info = find_pdata_range_for_pc(data, image_base, sections, begin)
        end = int(str(range_info.get("end_va")), 16) if range_info.get("end_va") else begin + 0x40
        blob = read_bytes(data, image_base, sections, begin, max(0, end - begin))
        primitive_name = str(spec["primitive"])
        wrappers[name] = {
            "function_begin_va": range_info.get("begin_va"),
            "function_end_va": range_info.get("end_va"),
            "primitive_name": primitive_name,
            "primitive_call": spec["primitive_call"],
            "submit_call": spec["submit_call"],
            "calls_primitive": any(
                ref.get("source_va") == spec["primitive_call"] for ref in rel32_refs.get(primitive_name, [])
            ),
            "calls_state_submit": spec["submit_call"] in submit_sources,
            "success_state_code_two_failure_three": b"\x41\x8d\x50\x02" in blob
            and b"\x84\xc0" in blob
            and b"\x41\x8d\x50\x03" in blob,
            "zeroes_r8_for_state_submit": b"\x45\x33\xc0" in blob,
            "bytes_hex": blob.hex(),
        }

    update_begin = TARGETS["menu_task_update_wrapper"]
    update_range = find_pdata_range_for_pc(data, image_base, sections, update_begin)
    update_end = int(str(update_range.get("end_va")), 16) if update_range.get("end_va") else update_begin + 0x4D
    update_blob = read_bytes(data, image_base, sections, update_begin, max(0, update_end - update_begin))
    update_context = {
        "function_begin_va": update_range.get("begin_va"),
        "function_end_va": update_range.get("end_va"),
        "calls_delta_pump": any(ref.get("source_va") == "0x14082a106" for ref in rel32_refs.get("save_load_pump_delta", [])),
        "calls_default_pump": any(ref.get("source_va") == "0x14082a10d" for ref in rel32_refs.get("save_load_pump_default", [])),
        "branches_to_default_on_nonpositive_plus8": b"\xf3\x0f\x10\x41\x08" in update_blob
        and b"\x0f\x2f\xc1" in update_blob
        and b"\x76\x07" in update_blob,
        "return_one_submits_state_one": b"\x83\xf8\x01" in update_blob
        and b"\x8b\xd0" in update_blob,
        "return_zero_submits_state_two_else_three": b"\xba\x02\x00\x00\x00" in update_blob
        and b"\x85\xc0" in update_blob
        and b"\xba\x03\x00\x00\x00" in update_blob,
        "calls_state_submit": "0x14082a12f" in submit_sources,
        "zeroes_r8_for_state_submit": b"\x45\x33\xc0" in update_blob,
        "bytes_hex": update_blob.hex(),
    }

    menu_load_submit_sources = sorted(
        [
            "0x14082baa7",
            "0x14082bae8",
            "0x14082bb20",
            "0x14082a12f",
        ],
        key=lambda value: int(value, 16),
    )
    state_helper_specs = {
        "clear": ("menu_task_state_clear_7a91f0", 9),
        "compare": ("menu_task_state_compare_7a9200", 7),
        "is_two": ("menu_task_state_is_two_7a9210", 7),
        "payload_ptr": ("menu_task_state_payload_ptr_7a9220", 13),
        "is_empty": ("menu_task_state_is_empty_7a9230", 19),
    }
    state_helper_family: dict[str, dict[str, Any]] = {}
    for helper_name, (target_name, size) in state_helper_specs.items():
        helper_va = TARGETS[target_name]
        refs = rel32_refs.get(target_name, [])
        state_helper_family[helper_name] = {
            "function_va": f"0x{helper_va:x}",
            "bytes_hex": read_bytes(data, image_base, sections, helper_va, size).hex(),
            "caller_count": len(refs),
            "caller_sources": [str(ref.get("source_va")) for ref in refs],
        }

    def infer_state_submit_code(blob: bytes, begin_va: int, call_source_va: int) -> int | str | None:
        call_offset = call_source_va - begin_va
        prefix = blob[max(0, call_offset - 24) : call_offset]
        for state_code in (1, 2, 3):
            if b"\xba" + struct.pack("<I", state_code) in prefix:
                return state_code
            if bytes([0x41, 0x8D, 0x50, state_code]) in prefix:
                return state_code
        if b"\x8b\xd0" in prefix:
            return "eax_return_value"
        return None

    helper_sources_by_role = {
        helper_name: set(context.get("caller_sources", [])) for helper_name, context in state_helper_family.items()
    }
    expected_terminal_compare_sources = {"0x14082007d", "0x140826fdc", "0x14082703d", "0x140834ad6"}
    expected_empty_check_sources = {"0x14082008a"}
    expected_consumer_sources = expected_terminal_compare_sources | expected_empty_check_sources
    consumer_contexts_by_begin: dict[int, dict[str, Any]] = {}
    for source_text in sorted(expected_consumer_sources, key=lambda value: int(value, 16)):
        source_va = int(source_text, 16)
        range_info = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = range_info.get("begin_va")
        end_text = range_info.get("end_va")
        if begin_text is None or end_text is None:
            continue
        begin_va = int(begin_text, 16)
        end_va = int(end_text, 16)
        blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
        context = consumer_contexts_by_begin.setdefault(
            begin_va,
            {
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "helper_calls": [],
                "state_submit_calls": [],
                "bytes_hex": blob.hex(),
            },
        )
        for helper_name, caller_sources in helper_sources_by_role.items():
            if source_text in caller_sources:
                context["helper_calls"].append({"helper": helper_name, "source_va": source_text})
        existing_submit_sources = {call["source_va"] for call in context["state_submit_calls"]}
        for ref in state_submit_refs:
            submit_source_text = str(ref.get("source_va"))
            if submit_source_text in existing_submit_sources:
                continue
            submit_source_va = int(submit_source_text, 16)
            if begin_va <= submit_source_va < end_va:
                context["state_submit_calls"].append(
                    {
                        "source_va": submit_source_text,
                        "inferred_state_code": infer_state_submit_code(blob, begin_va, submit_source_va),
                    }
                )
    state_code_consumer_contexts = [
        consumer_contexts_by_begin[key] for key in sorted(consumer_contexts_by_begin)
    ]
    consumer_submit_codes = {
        call.get("inferred_state_code")
        for context in state_code_consumer_contexts
        for call in context.get("state_submit_calls", [])
        if isinstance(call.get("inferred_state_code"), int)
    }
    state_consumer_vtable_specs = {
        "delay_gate_consumer_81ff10": {
            "vtable_base": 0x142AC3D90,
            "constructor": 0x14081DFA0,
            "constructor_target_name": "menu_state_delay_consumer_ctor_81dfa0",
            "update": 0x14081FF10,
            "update_slot_offset": 0x10,
        },
        "bridge_consumer_826f90": {
            "vtable_base": 0x142AC7098,
            "constructor": 0x140823D30,
            "constructor_target_name": "menu_state_bridge_consumer_ctor_823d30",
            "update": 0x140826F90,
            "update_slot_offset": 0x10,
        },
        "payload_consumer_8349c0": {
            "vtable_base": 0x142ACB1E8,
            "constructor": 0x14082EFE0,
            "constructor_target_name": "menu_state_payload_consumer_ctor_82efe0",
            "update": 0x1408349C0,
            "update_slot_offset": 0x10,
        },
    }
    state_code_consumer_vtables: dict[str, dict[str, Any]] = {}
    for name, spec in state_consumer_vtable_specs.items():
        vtable_base = int(spec["vtable_base"])
        constructor_va = int(spec["constructor"])
        update_va = int(spec["update"])
        update_slot_va = vtable_base + int(spec["update_slot_offset"])
        update_slot_value = read_qword_value(data, image_base, sections, update_slot_va)
        constructor_range = find_pdata_range_for_pc(data, image_base, sections, constructor_va)
        update_range = find_pdata_range_for_pc(data, image_base, sections, update_va)
        constructor_begin = int(str(constructor_range.get("begin_va") or f"0x{constructor_va:x}"), 16)
        constructor_end = int(str(constructor_range.get("end_va") or f"0x{constructor_va + 0x40:x}"), 16)
        update_begin = int(str(update_range.get("begin_va") or f"0x{update_va:x}"), 16)
        update_end = int(str(update_range.get("end_va") or f"0x{update_va + 0x40:x}"), 16)
        constructor_blob = read_bytes(data, image_base, sections, constructor_begin, max(0, constructor_end - constructor_begin))
        update_blob = read_bytes(data, image_base, sections, update_begin, max(0, update_end - update_begin))
        constructor_call_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(str(spec["constructor_target_name"]), []):
            source_text = str(ref.get("source_va"))
            source_va = int(source_text, 16)
            caller_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            caller_begin_text = caller_range.get("begin_va")
            caller_end_text = caller_range.get("end_va")
            if caller_begin_text is None or caller_end_text is None:
                continue
            caller_begin = int(caller_begin_text, 16)
            caller_end = int(caller_end_text, 16)
            caller_blob = read_bytes(data, image_base, sections, caller_begin, max(0, caller_end - caller_begin))
            if b"\xb9\x48\x0a\x00\x00" in caller_blob:
                allocation_size_hint = "0xa48"
            elif b"\x8d\x4a\x60" in caller_blob:
                allocation_size_hint = "0x68"
            elif b"\x8d\x4a\x28" in caller_blob:
                allocation_size_hint = "0x30"
            else:
                allocation_size_hint = None
            constructor_call_contexts.append(
                {
                    "source_va": source_text,
                    "function_begin_va": caller_begin_text,
                    "function_end_va": caller_end_text,
                    "allocation_size_hint": allocation_size_hint,
                    "loads_xmm2_delay_argument": b"\xf3\x0f\x10" in caller_blob or b"\x0f\x28\xd6" in caller_blob,
                    "calls_task_enqueue": any(
                        caller_begin <= int(str(row.get("source_va")), 16) < caller_end
                        for row in rel32_refs.get("task_enqueue_7a7b60", [])
                    ),
                    "calls_task_enqueue_link": any(
                        caller_begin <= int(str(row.get("source_va")), 16) < caller_end
                        for row in rel32_refs.get("task_enqueue_link_7a7bb0", [])
                    ),
                    "bytes_hex": caller_blob.hex(),
                }
            )
        consumer_context = consumer_contexts_by_begin.get(update_begin, {})
        helper_names = [str(call.get("helper")) for call in consumer_context.get("helper_calls", [])]
        submit_codes = [call.get("inferred_state_code") for call in consumer_context.get("state_submit_calls", [])]
        role_predicates = {
            "uses_terminal_compare": "compare" in helper_names,
            "uses_empty_zero_state_check": "is_empty" in helper_names,
            "submits_pending_state_one": 1 in submit_codes,
            "submits_success_state_two": 2 in submit_codes,
            "notifies_owner_virtual_60_after_success": b"\xff\x50\x60" in update_blob,
            "constructor_stores_large_delay_threshold": b"\xf3\x0f\x11\xb7\x3c\x0a\x00\x00" in constructor_blob,
            "constructor_stores_gate_flag": b"\x40\x88\xb7\x40\x0a\x00\x00" in constructor_blob,
            "constructor_clears_local_state_1c": b"\x48\x8d\x4b\x1c" in constructor_blob
            and any(ref.get("source_va") == "0x140823d8c" for ref in rel32_refs.get("menu_task_state_clear_7a91f0", [])),
            "constructor_stores_bridge_delay_24": b"\xf3\x0f\x11\x73\x24" in constructor_blob,
            "update_writes_result_to_owner_138": b"\x89\x8a\x38\x01\x00\x00" in update_blob,
            "constructor_stores_payload_fields": all(
                needle in constructor_blob
                for needle in [b"\x48\x89\x5e\x10", b"\x89\x7e\x18", b"\x48\x89\x4e\x20"]
            ),
        }
        if name == "delay_gate_consumer_81ff10":
            role = "terminal_delay_gate_success_notifier"
            role_mapped = bool(
                role_predicates["uses_terminal_compare"]
                and role_predicates["uses_empty_zero_state_check"]
                and role_predicates["submits_success_state_two"]
                and role_predicates["notifies_owner_virtual_60_after_success"]
                and role_predicates["constructor_stores_large_delay_threshold"]
                and role_predicates["constructor_stores_gate_flag"]
            )
        elif name == "bridge_consumer_826f90":
            role = "pending_bridge_until_terminal_compare"
            role_mapped = bool(
                helper_names.count("compare") >= 2
                and role_predicates["submits_pending_state_one"]
                and role_predicates["constructor_clears_local_state_1c"]
                and role_predicates["constructor_stores_bridge_delay_24"]
            )
        else:
            role = "payload_result_pending_until_terminal_compare"
            role_mapped = bool(
                role_predicates["uses_terminal_compare"]
                and role_predicates["submits_pending_state_one"]
                and role_predicates["update_writes_result_to_owner_138"]
                and role_predicates["constructor_stores_payload_fields"]
            )
        state_code_consumer_vtables[name] = {
            "vtable_base_va": f"0x{vtable_base:x}",
            "vtable_entries": read_qwords(data, image_base, sections, vtable_base, 4),
            "constructor_va": f"0x{constructor_va:x}",
            "constructor_range": constructor_range,
            "update_va": f"0x{update_va:x}",
            "update_range": update_range,
            "update_slot_va": f"0x{update_slot_va:x}",
            "update_slot_value_va": f"0x{update_slot_value:x}" if update_slot_value is not None else None,
            "constructor_stores_vtable_base": function_has_rip_lea_to(
                data, image_base, sections, constructor_va, vtable_base, max(0x60, constructor_end - constructor_begin)
            ),
            "update_slot_matches_consumer": update_slot_value == update_va,
            "role": role,
            "role_mapped": role_mapped,
            "role_predicates": role_predicates,
            "constructor_call_contexts": constructor_call_contexts,
            "constructor_bytes_hex": constructor_blob.hex(),
            "update_bytes_hex": update_blob.hex(),
        }
    return {
        "state_submit_helper": {
            "function_va": f"0x{state_submit_va:x}",
            "bytes_hex": state_submit_blob.hex(),
            "stores_edx_to_rcx_state0": state_submit_blob.startswith(b"\x89\x11"),
            "returns_rcx_in_rax": b"\x48\x8b\xc1" in state_submit_blob,
            "stores_r8d_to_rcx_plus4": b"\x44\x89\x41\x04" in state_submit_blob,
            "caller_count": len(state_submit_refs),
            "menu_load_submit_sources": menu_load_submit_sources,
            "menu_load_sources_are_subset": set(menu_load_submit_sources).issubset(state_submit_sources),
        },
        "state_helper_family": state_helper_family,
        "state_code_consumer_contexts": state_code_consumer_contexts,
        "state_code_consumer_vtables": state_code_consumer_vtables,
        "state_code_semantics": {
            "pending_state_code": 1,
            "success_state_code": 2,
            "failure_state_code": 3,
            "terminal_compare_helper": "state > 1",
            "empty_helper": "state == 0 and payload_ptr == 0",
            "consumer_submit_codes": sorted(consumer_submit_codes),
        },
        "wrappers": wrappers,
        "update_wrapper": update_context,
        "all_immediate_wrappers_call_primitives_and_submit_state": all(
            wrapper.get("calls_primitive")
            and wrapper.get("calls_state_submit")
            and wrapper.get("success_state_code_two_failure_three")
            for wrapper in wrappers.values()
        ),
        "pump_update_submits_state_from_pump_return": bool(
            update_context.get("calls_state_submit")
            and update_context.get("return_one_submits_state_one")
            and update_context.get("return_zero_submits_state_two_else_three")
        ),
        "pump_update_selects_delta_or_default_by_delay": bool(
            update_context.get("calls_delta_pump")
            and update_context.get("calls_default_pump")
            and update_context.get("branches_to_default_on_nonpositive_plus8")
        ),
        "state_submit_helper_is_simple_field_store": bool(
            state_submit_blob.startswith(b"\x89\x11\x48\x8b\xc1\x44\x89\x41\x04\xc3")
        ),
        "state_submit_helper_is_generic_broad_fan_in": len(state_submit_refs) > 600,
        "menu_load_state_submit_sources_are_exact_subset": set(menu_load_submit_sources).issubset(state_submit_sources),
        "state_helper_family_bodies_mapped": state_helper_family.get("clear", {}).get("bytes_hex") == "33c0488901488bc1c3"
        and state_helper_family.get("compare", {}).get("bytes_hex") == "8339010f97c0c3"
        and state_helper_family.get("is_two", {}).get("bytes_hex") == "8339020f94c0c3"
        and state_helper_family.get("payload_ptr", {}).get("bytes_hex") == "33c04839010f95c048034130c3"
        and state_helper_family.get("is_empty", {}).get("bytes_hex") == "48837930007509488339007503b001c332c0c3",
        "state_helper_family_ref_counts_mapped": {
            "clear": 4,
            "compare": 49,
            "is_two": 11,
            "payload_ptr": 1,
            "is_empty": 36,
        }.items()
        <= {name: int(context.get("caller_count", -1)) for name, context in state_helper_family.items()}.items(),
        "state_helper_family_timed_queue_consumer_mapped": "0x1407a9696"
        in set(state_helper_family.get("compare", {}).get("caller_sources", [])),
        "state_code_terminal_compare_consumers_mapped": expected_terminal_compare_sources.issubset(
            helper_sources_by_role.get("compare", set())
        ),
        "state_code_empty_check_consumer_mapped": expected_empty_check_sources.issubset(
            helper_sources_by_role.get("is_empty", set())
        ),
        "state_code_pending_success_failure_semantics_mapped": bool(
            {1, 2}.issubset(consumer_submit_codes)
            and update_context.get("return_one_submits_state_one")
            and update_context.get("return_zero_submits_state_two_else_three")
            and all(
                wrapper.get("success_state_code_two_failure_three")
                for wrapper in wrappers.values()
            )
        ),
        "state_code_consumer_vtables_mapped": all(
            context.get("constructor_stores_vtable_base") and context.get("update_slot_matches_consumer")
            for context in state_code_consumer_vtables.values()
        ),
        "state_code_consumer_roles_classified": all(
            context.get("role_mapped") for context in state_code_consumer_vtables.values()
        ),
        "state_code_consumer_constructor_callers_mapped": bool(
            len(state_code_consumer_vtables.get("delay_gate_consumer_81ff10", {}).get("constructor_call_contexts", [])) == 1
            and state_code_consumer_vtables["delay_gate_consumer_81ff10"]["constructor_call_contexts"][0].get(
                "allocation_size_hint"
            )
            == "0xa48"
            and len(state_code_consumer_vtables.get("bridge_consumer_826f90", {}).get("constructor_call_contexts", [])) == 1
            and state_code_consumer_vtables["bridge_consumer_826f90"]["constructor_call_contexts"][0].get(
                "allocation_size_hint"
            )
            == "0x68"
            and len(state_code_consumer_vtables.get("payload_consumer_8349c0", {}).get("constructor_call_contexts", [])) == 1
            and state_code_consumer_vtables["payload_consumer_8349c0"]["constructor_call_contexts"][0].get(
                "allocation_size_hint"
            )
            == "0x30"
            and state_code_consumer_vtables["payload_consumer_8349c0"]["constructor_call_contexts"][0].get(
                "calls_task_enqueue"
            )
            and state_code_consumer_vtables["payload_consumer_8349c0"]["constructor_call_contexts"][0].get(
                "calls_task_enqueue_link"
            )
        ),
    }


def read_function_ranges(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, dict[str, str | None]]:
    pdata = next((section for section in sections if section["name"] == ".pdata"), None)
    ranges: dict[str, dict[str, str | None]] = {}
    if pdata is None:
        return ranges
    raw_ptr = int(pdata["raw_ptr"])
    raw_size = int(pdata["raw_size"])
    for name, pc in FUNCTION_RANGE_PROBES.items():
        match = None
        for offset in range(raw_ptr, raw_ptr + raw_size - 11, 12):
            begin_rva, end_rva, unwind_rva = struct.unpack_from("<III", data, offset)
            if begin_rva <= pc - image_base < end_rva:
                match = {
                    "pc_va": f"0x{pc:x}",
                    "begin_va": f"0x{image_base + begin_rva:x}",
                    "end_va": f"0x{image_base + end_rva:x}",
                    "unwind_va": f"0x{image_base + unwind_rva:x}",
                }
                break
        ranges[name] = match or {"pc_va": f"0x{pc:x}", "begin_va": None, "end_va": None, "unwind_va": None}
    return ranges


def find_pdata_range_for_pc(
    data: bytes, image_base: int, sections: list[dict[str, int | str]], pc: int
) -> dict[str, str | None]:
    pdata = next((section for section in sections if section["name"] == ".pdata"), None)
    if pdata is None:
        return {"pc_va": f"0x{pc:x}", "begin_va": None, "end_va": None, "unwind_va": None}
    raw_ptr = int(pdata["raw_ptr"])
    raw_size = int(pdata["raw_size"])
    for offset in range(raw_ptr, raw_ptr + raw_size - 11, 12):
        begin_rva, end_rva, unwind_rva = struct.unpack_from("<III", data, offset)
        if begin_rva <= pc - image_base < end_rva:
            return {
                "pc_va": f"0x{pc:x}",
                "begin_va": f"0x{image_base + begin_rva:x}",
                "end_va": f"0x{image_base + end_rva:x}",
                "unwind_va": f"0x{image_base + unwind_rva:x}",
            }
    return {"pc_va": f"0x{pc:x}", "begin_va": None, "end_va": None, "unwind_va": None}


def scan_rip_relative_refs_to_va(
    data: bytes, image_base: int, sections: list[dict[str, int | str]], target_va: int
) -> list[dict[str, Any]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    patterns = [
        (b"\x80\x3d", 2, 7, "cmpb_m8_imm8"),
        (b"\xc6\x05", 2, 7, "movb_m8_imm8"),
        (b"\x48\x8b\x05", 3, 7, "movq_load_rax"),
        (b"\x48\x8b\x0d", 3, 7, "movq_load_rcx"),
        (b"\x48\x8b\x15", 3, 7, "movq_load_rdx"),
        (b"\x48\x8b\x1d", 3, 7, "movq_load_rbx"),
        (b"\x48\x8b\x35", 3, 7, "movq_load_rsi"),
        (b"\x48\x8b\x3d", 3, 7, "movq_load_rdi"),
        (b"\x4c\x8b\x05", 3, 7, "movq_load_r8"),
        (b"\x4c\x8b\x0d", 3, 7, "movq_load_r9"),
        (b"\x4c\x8b\x15", 3, 7, "movq_load_r10"),
        (b"\x4c\x8b\x1d", 3, 7, "movq_load_r11"),
        (b"\x48\x89\x05", 3, 7, "movq_store"),
        (b"\x48\x8d\x05", 3, 7, "lea_rax"),
        (b"\x48\x8d\x0d", 3, 7, "lea_rcx"),
        (b"\x48\x8d\x15", 3, 7, "lea_rdx"),
        (b"\x48\x8d\x1d", 3, 7, "lea_rbx"),
        (b"\x48\x8d\x35", 3, 7, "lea_rsi"),
        (b"\x48\x8d\x3d", 3, 7, "lea_rdi"),
        (b"\x4c\x8d\x05", 3, 7, "lea_r8"),
        (b"\x4c\x8d\x0d", 3, 7, "lea_r9"),
        (b"\x4c\x8d\x15", 3, 7, "lea_r10"),
        (b"\x4c\x8d\x1d", 3, 7, "lea_r11"),
    ]
    refs: list[dict[str, Any]] = []
    for needle, disp_offset, instruction_size, instruction in patterns:
        start = 0
        while True:
            index = text_data.find(needle, start)
            if index < 0:
                break
            disp = struct.unpack_from("<i", text_data, index + disp_offset)[0]
            source_va = text_va + index
            resolved_va = source_va + instruction_size + disp
            if resolved_va == target_va:
                function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
                refs.append(
                    {
                        "source_va": f"0x{source_va:x}",
                        "source_rva": f"0x{source_va - image_base:x}",
                        "target_va": f"0x{target_va:x}",
                        "target_rva": f"0x{target_va - image_base:x}",
                        "instruction": instruction,
                        "bytes_hex": text_data[index : index + instruction_size].hex(),
                        "function_begin_va": function_range.get("begin_va"),
                        "function_end_va": function_range.get("end_va"),
                    }
                )
            start = index + 1
    return sorted(refs, key=lambda item: int(str(item["source_va"]), 16))


def scan_entry_vtable_rip_refs(
    data: bytes, image_base: int, sections: list[dict[str, int | str]]
) -> dict[str, list[dict[str, Any]]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    by_target = {addr: name for name, addr in ENTRY_HELPER_VTABLE_BASES.items()}
    refs: dict[str, list[dict[str, Any]]] = {name: [] for name in ENTRY_HELPER_VTABLE_BASES}
    for index in range(0, max(0, len(text_data) - 7)):
        if text_data[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
            continue
        disp = struct.unpack_from("<i", text_data, index + 3)[0]
        source_va = text_va + index
        target_va = source_va + 7 + disp
        target_name = by_target.get(target_va)
        if target_name is None:
            continue
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        refs[target_name].append(
            {
                "source_va": f"0x{source_va:x}",
                "source_rva": f"0x{source_va - image_base:x}",
                "target_va": f"0x{target_va:x}",
                "target_rva": f"0x{target_va - image_base:x}",
                "bytes_hex": text_data[index : index + 7].hex(),
                "function_begin_va": function_range.get("begin_va"),
                "function_end_va": function_range.get("end_va"),
            }
        )
    return refs


def scan_selector_entry_vtable_rip_refs(
    data: bytes, image_base: int, sections: list[dict[str, int | str]]
) -> dict[str, list[dict[str, Any]]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    by_target = {addr: name for name, addr in SELECTOR_ENTRY_VTABLE_BASES.items()}
    refs: dict[str, list[dict[str, Any]]] = {name: [] for name in SELECTOR_ENTRY_VTABLE_BASES}
    for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
        index = -1
        while True:
            index = text_data.find(pattern, index + 1)
            if index < 0 or index + 7 > len(text_data):
                break
            disp = struct.unpack_from("<i", text_data, index + 3)[0]
            source_va = text_va + index
            target_va = source_va + 7 + disp
            target_name = by_target.get(target_va)
            if target_name is None:
                continue
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            refs[target_name].append(
                {
                    "source_va": f"0x{source_va:x}",
                    "source_rva": f"0x{source_va - image_base:x}",
                    "target_va": f"0x{target_va:x}",
                    "target_rva": f"0x{target_va - image_base:x}",
                    "bytes_hex": text_data[index : index + 7].hex(),
                    "function_begin_va": function_range.get("begin_va"),
                    "function_end_va": function_range.get("end_va"),
                }
            )
    return refs


def scan_entry_family_descriptor_rip_refs(
    data: bytes, image_base: int, sections: list[dict[str, int | str]]
) -> dict[str, list[dict[str, Any]]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    by_target = {addr: name for name, addr in ENTRY_FAMILY_DESCRIPTOR_VTABLE_BASES.items()}
    refs: dict[str, list[dict[str, Any]]] = {name: [] for name in ENTRY_FAMILY_DESCRIPTOR_VTABLE_BASES}
    for index in range(0, max(0, len(text_data) - 7)):
        if text_data[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
            continue
        disp = struct.unpack_from("<i", text_data, index + 3)[0]
        source_va = text_va + index
        target_va = source_va + 7 + disp
        target_name = by_target.get(target_va)
        if target_name is None:
            continue
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        refs[target_name].append(
            {
                "source_va": f"0x{source_va:x}",
                "source_rva": f"0x{source_va - image_base:x}",
                "target_va": f"0x{target_va:x}",
                "target_rva": f"0x{target_va - image_base:x}",
                "bytes_hex": text_data[index : index + 7].hex(),
                "function_begin_va": function_range.get("begin_va"),
                "function_end_va": function_range.get("end_va"),
            }
        )
    return refs


def read_entry_family_builder_call_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    contexts: list[dict[str, Any]] = []
    for ref in rel32_refs.get("entry_family_builder_8279e0", []):
        source_va = int(str(ref["source_va"]), 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        window_start = source_va - 0x60
        if begin_text is not None:
            window_start = max(window_start, int(begin_text, 16))
        window_end = source_va + 0x20
        if end_text is not None:
            window_end = min(window_end, int(end_text, 16))
        blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
        wrapper_target = None
        xmm2_mode = None
        xmm2_value = None
        for index in range(max(0, len(blob) - 7)):
            if blob[index : index + 3] == b"\x48\x8d\x05":
                disp = struct.unpack_from("<i", blob, index + 3)[0]
                target = window_start + index + 7 + disp
                if index + 11 <= len(blob) and blob[index + 7 : index + 11] == b"\x49\x89\x43\xb8":
                    wrapper_target = target
            if blob[index : index + 4] == b"\xf3\x0f\x10\x15":
                disp = struct.unpack_from("<i", blob, index + 4)[0]
                float_va = window_start + index + 8 + disp
                file_offset = va_to_file_offset(image_base, sections, float_va)
                if file_offset is not None and file_offset + 4 <= len(data):
                    xmm2_mode = "rip_float"
                    xmm2_value = struct.unpack_from("<f", data, file_offset)[0]
        call_offset = source_va - window_start
        contexts.append(
            {
                "source_va": ref["source_va"],
                "target_va": ref["target_va"],
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "context_start_va": f"0x{window_start:x}",
                "context_bytes_hex": blob.hex(),
                "wrapper_target_va": f"0x{wrapper_target:x}" if wrapper_target is not None else None,
                "xmm2_mode": xmm2_mode,
                "xmm2_value": xmm2_value,
                "moves_rbx_to_rcx_before_call": call_offset >= 3 and blob[call_offset - 3 : call_offset] == b"\x48\x8b\xcb",
                "uses_stack_descriptor_as_rdx_before_call": call_offset >= 7 and blob[call_offset - 7 : call_offset - 3] == b"\x49\x8d\x53\xb0",
            }
        )
    return contexts


def read_entry_selector_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    contexts: list[dict[str, Any]] = []
    for ref in rel32_refs.get("entry_selector_824b50", []):
        source_va = int(str(ref["source_va"]), 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        window_start = source_va - 0x30
        if begin_text is not None:
            window_start = max(window_start, int(begin_text, 16))
        window_end = source_va + 0x10
        if end_text is not None:
            window_end = min(window_end, int(end_text, 16))
        blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
        function_blob = b""
        if begin_text is not None and end_text is not None:
            function_blob = read_bytes(data, image_base, sections, int(begin_text, 16), int(end_text, 16) - int(begin_text, 16))
        r8_immediate = None
        for index in range(max(0, len(blob) - 6)):
            if blob[index : index + 2] == b"\x41\xb8":
                r8_immediate = struct.unpack_from("<I", blob, index + 2)[0]
        has_rip_vtable_lea = False
        start_for_function_blob = int(begin_text, 16) if begin_text is not None else 0
        for index in range(max(0, len(function_blob) - 7)):
            if function_blob[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
                continue
            disp = struct.unpack_from("<i", function_blob, index + 3)[0]
            target = start_for_function_blob + index + 7 + disp
            if 0x142AC7000 <= target <= 0x142AC7A00:
                has_rip_vtable_lea = True
        contexts.append(
            {
                "source_va": ref["source_va"],
                "target_va": ref["target_va"],
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "context_start_va": f"0x{window_start:x}",
                "context_bytes_hex": blob.hex(),
                "r8_immediate": r8_immediate,
                "entry_function_has_rip_vtable_lea": has_rip_vtable_lea,
                "entry_function_calls_entry_selector": source_va in range(int(begin_text or "0", 16), int(end_text or "0", 16)) if begin_text and end_text else False,
            }
        )
    return contexts


def read_entry_selector_parent_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in ENTRY_SELECTOR_TARGET_NAMES:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x40
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                    "forwards_r8_plus_0x50_or_null": b"\x49\x83\xc0\x50" in blob
                    and b"\x4c\x0f\x44\xc1" in blob,
                    "forwards_rcx_to_rdx": b"\x48\x8b\xd3" in blob,
                    "forwards_r9_to_rcx": b"\x49\x8b\xc9" in blob,
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_selector_chain_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    target_names: list[str],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in target_names:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x40
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                    "forwards_r8_to_rdx": b"\x49\x8b\xd0" in blob,
                    "forwards_r9_to_r8": b"\x4d\x8b\xc1" in blob,
                    "preserves_rcx_in_rbx": b"\x48\x8b\xd9" in blob,
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_entry_level_helper_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in ENTRY_LEVEL_TARGET_NAMES:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x50
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            call_offset = source_va - window_start
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                    "passes_incoming_r8_to_r9": b"\x4d\x8b\xc8" in blob[:call_offset],
                    "passes_task_plus8_as_r8": b"\x4c\x8d\x41\x08" in blob[:call_offset],
                    "passes_stack_flag_as_edx": b"\x0f\xb6\x54\x24\x48" in blob[:call_offset],
                    "moves_incoming_rdx_to_rbx": b"\x48\x8b\xda" in blob[:call_offset],
                    "calls_with_rbx_as_rcx": call_offset >= 3 and blob[call_offset - 3 : call_offset] == b"\x48\x8b\xcb",
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_selector_entry_helper_vtable_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    absolute_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in SELECTOR_ENTRY_HELPER_TARGET_NAMES:
        target_contexts: list[dict[str, Any]] = []
        for ref in absolute_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            vtable_base = source_va - 0x10
            plus8_accessor_va = read_qword_value(data, image_base, sections, source_va - 0x20)
            ctor_va = read_qword_value(data, image_base, sections, vtable_base)
            copy_or_clone_va = read_qword_value(data, image_base, sections, vtable_base + 8)
            helper_va = read_qword_value(data, image_base, sections, source_va)
            destructor_va = read_qword_value(data, image_base, sections, vtable_base + 0x20)
            ctor_thunk_target_va = function_tail_jump_target(data, image_base, sections, ctor_va)
            plus8_accessor_bytes = read_bytes(data, image_base, sections, plus8_accessor_va or 0, 5)
            ctor_thunk_target_bytes = read_bytes(data, image_base, sections, ctor_thunk_target_va or 0, 0x80)
            constructor_stores_vtable_base = function_has_rip_lea_to(data, image_base, sections, ctor_va, vtable_base)
            constructor_thunk_target_stores_vtable_base = function_has_rip_lea_to(
                data, image_base, sections, ctor_thunk_target_va, vtable_base, size=0x90
            )
            context_rows = read_qwords(data, image_base, sections, source_va - 0x30, 13)
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "section": ref["section"],
                    "target_va": ref["target_va"],
                    "vtable_base_va": f"0x{vtable_base:x}",
                    "task_plus8_accessor_slot_va": f"0x{source_va - 0x20:x}",
                    "task_plus8_accessor_va": f"0x{plus8_accessor_va:x}" if plus8_accessor_va is not None else None,
                    "constructor_slot_va": f"0x{vtable_base:x}",
                    "constructor_va": f"0x{ctor_va:x}" if ctor_va is not None else None,
                    "constructor_thunk_target_va": f"0x{ctor_thunk_target_va:x}" if ctor_thunk_target_va is not None else None,
                    "copy_or_clone_slot_va": f"0x{vtable_base + 8:x}",
                    "copy_or_clone_va": f"0x{copy_or_clone_va:x}" if copy_or_clone_va is not None else None,
                    "helper_slot_va": f"0x{source_va:x}",
                    "helper_va": f"0x{helper_va:x}" if helper_va is not None else None,
                    "destructor_slot_va": f"0x{vtable_base + 0x20:x}",
                    "destructor_va": f"0x{destructor_va:x}" if destructor_va is not None else None,
                    "helper_matches_target": helper_va == TARGETS[target_name],
                    "task_plus8_accessor_returns_rcx_plus8": plus8_accessor_bytes == b"\x48\x8d\x41\x08\xc3",
                    "constructor_stores_vtable_base": constructor_stores_vtable_base,
                    "constructor_thunk_target_stores_vtable_base": constructor_thunk_target_stores_vtable_base,
                    "constructor_or_thunk_stores_vtable_base": constructor_stores_vtable_base or constructor_thunk_target_stores_vtable_base,
                    "constructor_thunk_target_allocates_0xc0": b"\xb9\xc0\x00\x00\x00" in ctor_thunk_target_bytes,
                    "constructor_thunk_target_copies_task_plus8": b"\x48\x8d\x4b\x08" in ctor_thunk_target_bytes
                    and b"\x48\x8d\x57\x08" in ctor_thunk_target_bytes,
                    "copy_or_clone_stores_vtable_base": function_has_rip_lea_to(data, image_base, sections, copy_or_clone_va, vtable_base),
                    "destructor_resets_vtable_base": function_has_rip_lea_to(data, image_base, sections, destructor_va, vtable_base),
                    "vtable_context_start_va": f"0x{source_va - 0x30:x}",
                    "vtable_context_qwords": context_rows,
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_selector6_builder_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    start_va, end_va = SELECTOR_BUILDER_CONTEXT
    blob = read_bytes(data, image_base, sections, start_va, end_va - start_va)
    function_range = find_pdata_range_for_pc(data, image_base, sections, start_va)
    call_target_names = [
        "selector_builder_local_wrapper_744d10",
        "selector_builder_input_key_7600a0",
        "selector_builder_context_init_78c950",
        "selector_builder_chain_key_7a91e0",
        "selector_builder_chain_append_7ccbb0",
        "selector_builder_chain_submit_78dac0",
        "selector_builder_context_cleanup_743700",
    ]
    calls_by_target: dict[str, list[str]] = {}
    for target_name in call_target_names:
        sources = [
            str(ref.get("source_va"))
            for ref in rel32_refs.get(target_name, [])
            if start_va <= int(str(ref.get("source_va")), 16) < end_va
        ]
        calls_by_target[target_name] = sorted(sources, key=lambda value: int(value, 16))

    lea_targets: list[dict[str, Any]] = []
    target_names_by_va = {
        **{addr: name for name, addr in SELECTOR_BUILDER_DESCRIPTOR_VTABLES.items()},
        **{addr: name for name, addr in SELECTOR_BUILDER_WRAPPER_TAGS.items()},
    }
    for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05", b"\x48\x8d\x15"):
        index = -1
        while True:
            index = blob.find(pattern, index + 1)
            if index < 0 or index + 7 > len(blob):
                break
            source_va = start_va + index
            target_va = source_va + 7 + struct.unpack_from("<i", blob, index + 3)[0]
            target_name = target_names_by_va.get(target_va)
            if target_name is None:
                continue
            lea_targets.append(
                {
                    "source_va": f"0x{source_va:x}",
                    "target_va": f"0x{target_va:x}",
                    "target_name": target_name,
                    "bytes_hex": blob[index : index + 7].hex(),
                }
            )
    lea_targets.sort(key=lambda row: int(str(row["source_va"]), 16))

    append_descriptor_by_source = {
        "0x140827f22": "selector_unknown_7488",
        "0x140827f5d": "selector_unknown_74c0",
        "0x140827f98": "selector3_entry_vtable_74f8",
        "0x140827fd3": "selector4_entry_vtable_7530",
        "0x140828011": "selector_unknown_7568",
        "0x140828051": "selector6_entry_vtable_75a0",
    }
    chain_append_order = [
        {
            "source_va": source,
            "descriptor_name": append_descriptor_by_source.get(source),
        }
        for source in calls_by_target["selector_builder_chain_append_7ccbb0"]
    ]

    local_wrapper_tags = [
        row["target_name"]
        for row in lea_targets
        if row["target_name"] in SELECTOR_BUILDER_WRAPPER_TAGS
    ]
    descriptor_vtables = [
        row["target_name"]
        for row in lea_targets
        if row["target_name"] in SELECTOR_BUILDER_DESCRIPTOR_VTABLES
    ]
    submit_source = calls_by_target["selector_builder_chain_submit_78dac0"][:1]
    submit_uses_incoming_rcx_owner = False
    if submit_source:
        source_va = int(submit_source[0], 16)
        submit_offset = source_va - start_va
        submit_uses_incoming_rcx_owner = submit_offset >= 6 and blob[submit_offset - 6 : submit_offset] == b"\x48\x8b\xc8\x49\x8b\xd5"

    return {
        "start_va": f"0x{start_va:x}",
        "end_va": f"0x{end_va:x}",
        "function_begin_va": function_range.get("begin_va"),
        "function_end_va": function_range.get("end_va"),
        "uses_incoming_r8_scratch": b"\x4c\x89\x44\x24\x40" in blob[:0x60],
        "moves_incoming_rdx_to_rbx": b"\x48\x8b\xda" in blob[:0x60],
        "moves_incoming_rcx_to_r13": b"\x4c\x8b\xe9" in blob[:0x60],
        "loads_incoming_rdx_plus_0x30": b"\x48\x8b\x42\x30" in blob[:0x80],
        "calls_by_target": calls_by_target,
        "lea_targets": lea_targets,
        "local_wrapper_tags": local_wrapper_tags,
        "descriptor_vtables": descriptor_vtables,
        "chain_append_order": chain_append_order,
        "submit_uses_incoming_rcx_owner": submit_uses_incoming_rcx_owner,
        "context_bytes_hex_prefix": blob[:0x120].hex(),
    }


def _first_call_context_for_target(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    target_name: str,
) -> dict[str, Any] | None:
    refs = rel32_refs.get(target_name, [])
    if not refs:
        return None
    ref = refs[0]
    source_va = int(str(ref["source_va"]), 16)
    function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
    begin_text = function_range.get("begin_va")
    end_text = function_range.get("end_va")
    window_start = source_va - 0x50
    if begin_text is not None:
        window_start = max(window_start, int(begin_text, 16))
    window_end = source_va + 0x20
    if end_text is not None:
        window_end = min(window_end, int(end_text, 16))
    blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
    return {
        "source_va": ref["source_va"],
        "target_va": ref["target_va"],
        "function_begin_va": begin_text,
        "function_end_va": end_text,
        "context_start_va": f"0x{window_start:x}",
        "context_bytes_hex": blob.hex(),
    }


def read_selector6_builder_direct_caller_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    start_va, end_va = SELECTOR_BUILDER_DIRECT_CALLER
    blob = read_bytes(data, image_base, sections, start_va, end_va - start_va)
    function_range = find_pdata_range_for_pc(data, image_base, sections, start_va)
    global_fast_path_flag = None
    for index in range(max(0, len(blob) - 7)):
        if blob[index : index + 3] != b"\x40\x38\x2d":
            continue
        target_va = start_va + index + 7 + struct.unpack_from("<i", blob, index + 3)[0]
        global_fast_path_flag = target_va
        break

    calls_by_target: dict[str, list[str]] = {}
    for target_name in [
        "selector6_builder_context_827bd0",
        "selector_builder_fallback_compose_828570",
        "task_enqueue_7a7b60",
    ]:
        sources = [
            str(ref.get("source_va"))
            for ref in rel32_refs.get(target_name, [])
            if start_va <= int(str(ref.get("source_va")), 16) < end_va
        ]
        calls_by_target[target_name] = sorted(sources, key=lambda value: int(value, 16))

    parent_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_direct_caller_825f70"
    )
    outer_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_parent_thunk_822460"
    )
    entry_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_outer_thunk_823950"
    )
    parent_blob = bytes.fromhex(parent_context["context_bytes_hex"]) if parent_context else b""
    outer_blob = bytes.fromhex(outer_context["context_bytes_hex"]) if outer_context else b""
    entry_blob = bytes.fromhex(entry_context["context_bytes_hex"]) if entry_context else b""

    return {
        "start_va": f"0x{start_va:x}",
        "end_va": f"0x{end_va:x}",
        "function_begin_va": function_range.get("begin_va"),
        "function_end_va": function_range.get("end_va"),
        "global_fast_path_flag_va": f"0x{global_fast_path_flag:x}" if global_fast_path_flag is not None else None,
        "captures_incoming_r8_in_rsi": b"\x49\x8b\xf0" in blob[:0x40],
        "captures_incoming_rdx_in_rbx": b"\x48\x8b\xda" in blob[:0x40],
        "captures_incoming_rcx_in_rdi": b"\x48\x8b\xf9" in blob[:0x40],
        "direct_call_arg_shuffle": {
            "rcx": "incoming_rdx_via_rbx" if b"\x48\x8b\xcb" in blob[:0x50] else None,
            "rdx": "incoming_r8_via_rsi" if b"\x48\x8b\xd6" in blob[:0x50] else None,
            "r8": "incoming_rcx" if b"\x4c\x8b\xc1" in blob[:0x50] else None,
        },
        "fallback_reads_incoming_rcx_plus_b0": b"\x48\x8b\x89\xb0\x00\x00\x00" in blob,
        "fallback_reads_incoming_rcx_plus_70_via_rdi": b"\x48\x8b\x4f\x70" in blob,
        "fallback_passes_stack_clones_to_compose": b"\x4c\x8d\x4c\x24\x40" in blob
        and b"\x4c\x8d\x84\x24\x80\x00\x00\x00" in blob
        and b"\x48\x8b\xd6" in blob,
        "calls_by_target": calls_by_target,
        "parent_context": parent_context,
        "parent_passes_incoming_rdx_as_child_rcx": b"\x48\x8b\xc8" in parent_blob,
        "parent_passes_incoming_rcx_as_child_rdx": b"\x48\x8b\xd1" in parent_blob,
        "outer_context": outer_context,
        "outer_preserves_args_to_parent": bool(outer_context and b"\x48\x8b\xd9" in outer_blob),
        "entry_context": entry_context,
        "entry_passes_incoming_r8_as_child_rdx": b"\x49\x8b\xd0" in entry_blob,
        "entry_passes_incoming_r9_as_child_r8": b"\x4d\x8b\xc1" in entry_blob,
        "entry_to_builder_arg_sources": {
            "rcx": "entry_incoming_rcx_owner",
            "rdx": "entry_incoming_r9",
            "r8": "entry_incoming_r8",
        },
        "context_bytes_hex_prefix": blob[:0x110].hex(),
    }


def read_selector6_builder_entry_owner_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    copy_ctor_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_entry_copy_ctor_8233c0"
    )
    copy_wrapper_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_entry_copy_wrapper_822f60"
    )
    owner_init_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_entry_owner_init_821b70"
    )
    owner_compose_context = _first_call_context_for_target(
        data, image_base, sections, rel32_refs, "selector6_builder_entry_owner_compose_828830"
    )

    copy_wrapper_blob = bytes.fromhex(copy_ctor_context["context_bytes_hex"]) if copy_ctor_context else b""
    owner_init_blob = bytes.fromhex(copy_wrapper_context["context_bytes_hex"]) if copy_wrapper_context else b""
    owner_compose_blob = bytes.fromhex(owner_init_context["context_bytes_hex"]) if owner_init_context else b""
    owner_parent_blob = bytes.fromhex(owner_compose_context["context_bytes_hex"]) if owner_compose_context else b""
    owner_compose_begin = int(owner_compose_context["function_begin_va"], 16) if owner_compose_context and owner_compose_context.get("function_begin_va") else 0
    owner_compose_end = int(owner_compose_context["function_end_va"], 16) if owner_compose_context and owner_compose_context.get("function_end_va") else owner_compose_begin
    owner_compose_body = read_bytes(data, image_base, sections, owner_compose_begin, max(0, owner_compose_end - owner_compose_begin))
    copy_ctor_va = TARGETS["selector6_builder_entry_copy_ctor_8233c0"]
    copy_ctor_range = find_pdata_range_for_pc(data, image_base, sections, copy_ctor_va)
    copy_ctor_body = read_bytes(data, image_base, sections, copy_ctor_va, 0x90)

    return {
        "copy_ctor_context": copy_ctor_context,
        "copy_ctor_body_range": copy_ctor_range,
        "copy_ctor_stores_vtable_base": function_has_rip_lea_to(
            data, image_base, sections, copy_ctor_va, SELECTOR_ENTRY_VTABLE_BASES["selector6_builder_entry_vtable_7648"], size=0x90
        ),
        "copy_ctor_copies_plus8_and_payload_ranges": b"\x48\x8d\x5a\x08" in copy_ctor_body
        and b"\x49\x8d\x50\x08" in copy_ctor_body
        and b"\x48\x8d\x4b\x08" in copy_ctor_body,
        "copy_wrapper_context": copy_wrapper_context,
        "copy_wrapper_allocates_0xc0": b"\xb9\xc0\x00\x00\x00" in copy_wrapper_blob,
        "copy_wrapper_passes_new_object_to_copy_ctor": b"\x48\x8b\xd3" in copy_wrapper_blob
        and b"\x4d\x8b\xc6" in copy_wrapper_blob
        and b"\x4c\x8b\xce" in copy_wrapper_blob,
        "copy_wrapper_installs_result_at_owner_plus_0x38": b"\x48\x89\x5f\x38" in copy_wrapper_blob,
        "owner_init_context": owner_init_context,
        "owner_init_clears_owner_plus_0x38": b"\x48\xc7\x41\x38\x00\x00\x00\x00" in owner_init_blob,
        "owner_init_passes_stack_descriptor_and_calls_copy_wrapper": b"\x4d\x8d\x43\xc8" in owner_init_blob
        and b"\x48\x8b\xd7" in owner_init_blob,
        "owner_init_calls_cleanup_after_copy_wrapper": bool(
            rel32_refs.get("selector6_builder_entry_owner_cleanup_823fe0")
            and any(ref.get("source_va") == "0x140821bcd" for ref in rel32_refs.get("selector6_builder_entry_owner_cleanup_823fe0", []))
        ),
        "owner_compose_context": owner_compose_context,
        "owner_compose_loads_two_stack_source_objects": b"\x48\x8b\xbd\x50\x01\x00\x00" in owner_compose_body
        and b"\x48\x8b\xb5\x58\x01\x00\x00" in owner_compose_body,
        "owner_compose_clones_two_source_plus_0x38_descriptors": b"\x48\x8b\x4e\x38" in owner_compose_body
        and b"\x48\x8b\x4f\x38" in owner_compose_body
        and b"\x48\x89\x85\xc8\x00\x00\x00" in owner_compose_body
        and b"\x48\x89\x45\xc0" in owner_compose_body,
        "owner_compose_passes_stack_clones_to_owner_init": b"\x4c\x8d\x8d\x90\x00\x00\x00" in owner_compose_body
        and b"\x4c\x8d\x45\x88" in owner_compose_body
        and b"\x49\x8b\xd4" in owner_compose_body
        and b"\x48\x8d\x4d\x10" in owner_compose_body,
        "owner_compose_calls_task_enqueue_three_times": [
            str(ref.get("source_va"))
            for ref in rel32_refs.get("task_enqueue_7a7b60", [])
            if owner_compose_begin <= int(str(ref.get("source_va")), 16) < owner_compose_end
        ]
        == ["0x140828a37", "0x140828a68", "0x140828ad4"],
        "owner_compose_builds_selector_container_vtable_76b8": function_has_rip_lea_to(
            data, image_base, sections, owner_compose_begin, 0x142AC76B8, size=0x140
        ),
        "owner_compose_body_range": {
            "begin_va": f"0x{owner_compose_begin:x}" if owner_compose_begin else None,
            "end_va": f"0x{owner_compose_end:x}" if owner_compose_end else None,
        },
        "call_sources": {
            "copy_ctor": [str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_entry_copy_ctor_8233c0", [])],
            "copy_wrapper": [str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_entry_copy_wrapper_822f60", [])],
            "owner_init": [str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_entry_owner_init_821b70", [])],
            "owner_compose": [str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_entry_owner_compose_828830", [])],
        },
    }


def read_selector6_owner_compose_parent_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    contexts: list[dict[str, Any]] = []
    target_lookup = {addr: name for name, addr in SELECTOR_OWNER_PARENT_TARGETS.items()}
    for ref in sorted(
        rel32_refs.get("selector6_builder_entry_owner_compose_parent_8288e0", []),
        key=lambda row: int(str(row.get("source_va")), 16),
    ):
        source_va = int(str(ref["source_va"]), 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        begin_va = int(begin_text, 16) if begin_text else source_va - 0x80
        end_va = int(end_text, 16) if end_text else source_va + 0x40
        blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
        call_offset = source_va - begin_va
        local_wrapper_calls = [
            str(call_ref.get("source_va"))
            for call_ref in rel32_refs.get("selector_builder_local_wrapper_744d10", [])
            if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
        ]
        input_key_calls = [
            str(call_ref.get("source_va"))
            for call_ref in rel32_refs.get("selector_builder_input_key_7600a0", [])
            if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
        ]
        input_guard_calls = [
            str(call_ref.get("source_va"))
            for call_ref in rel32_refs.get("selector_owner_parent_input_guard_765030", [])
            if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
        ]
        low_alloc_calls = [
            str(call_ref.get("source_va"))
            for call_ref in rel32_refs.get("task_alloc_selector_low_7a7250", [])
            if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
        ]
        lea_targets: list[dict[str, str]] = []
        for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05", b"\x48\x8d\x15"):
            index = -1
            while True:
                index = blob.find(pattern, index + 1)
                if index < 0 or index + 7 > len(blob):
                    break
                lea_source_va = begin_va + index
                target_va = lea_source_va + 7 + struct.unpack_from("<i", blob, index + 3)[0]
                target_name = target_lookup.get(target_va)
                if target_name is None:
                    continue
                lea_targets.append(
                    {
                        "source_va": f"0x{lea_source_va:x}",
                        "target_va": f"0x{target_va:x}",
                        "target_name": target_name,
                    }
                )
        call_window = blob[max(0, call_offset - 0x20) : min(len(blob), call_offset + 5)]
        local_wrapper_arg_pattern = b"\x4c\x8b\xcb\x4c\x8b\xc0\x48\x8b\xd7\x48\x8b\xce" in call_window
        input_key_arg_pattern = b"\x4c\x8b\xce\x4c\x8b\xc0\x48\x8b\xd7\x48\x8d\x4c\x24\x40" in call_window
        contexts.append(
            {
                "source_va": ref["source_va"],
                "target_va": ref["target_va"],
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "local_wrapper_calls": sorted(local_wrapper_calls, key=lambda value: int(value, 16)),
                "input_key_calls": sorted(input_key_calls, key=lambda value: int(value, 16)),
                "input_guard_calls": sorted(input_guard_calls, key=lambda value: int(value, 16)),
                "low_alloc_calls": sorted(low_alloc_calls, key=lambda value: int(value, 16)),
                "lea_targets": sorted(lea_targets, key=lambda row: int(row["source_va"], 16)),
                "passes_common_args_via_local_wrapper": local_wrapper_arg_pattern,
                "passes_common_args_via_input_key": input_key_arg_pattern,
                "call_window_bytes_hex": call_window.hex(),
            }
        )
    return contexts


def read_selector6_owner_variant_caller_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    vtable_lookup = {addr: name for name, addr in SELECTOR_OWNER_VARIANT_LOCAL_DESCRIPTOR_VTABLES.items()}
    for target_name in SELECTOR_OWNER_VARIANT_TARGET_NAMES:
        target_contexts: list[dict[str, Any]] = []
        for ref in sorted(rel32_refs.get(target_name, []), key=lambda row: int(str(row.get("source_va")), 16)):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            begin_va = int(begin_text, 16) if begin_text else source_va - 0x80
            end_va = int(end_text, 16) if end_text else source_va + 0x40
            blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
            call_offset = source_va - begin_va
            enqueue_calls = [
                str(call_ref.get("source_va"))
                for call_ref in rel32_refs.get("task_enqueue_7a7b60", [])
                if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
            ]
            enqueue_link_calls = [
                str(call_ref.get("source_va"))
                for call_ref in rel32_refs.get("task_enqueue_link_7a7bb0", [])
                if begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
            ]
            lea_vtables: list[dict[str, str]] = []
            for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
                index = -1
                while True:
                    index = blob.find(pattern, index + 1)
                    if index < 0 or index + 7 > len(blob):
                        break
                    lea_source_va = begin_va + index
                    target_va = lea_source_va + 7 + struct.unpack_from("<i", blob, index + 3)[0]
                    target_vtable_name = vtable_lookup.get(target_va)
                    if target_vtable_name is None:
                        continue
                    lea_vtables.append(
                        {
                            "source_va": f"0x{lea_source_va:x}",
                            "target_va": f"0x{target_va:x}",
                            "target_name": target_vtable_name,
                        }
                    )
            call_window = blob[max(0, call_offset - 0x40) : min(len(blob), call_offset + 5)]
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "enqueue_calls": sorted(enqueue_calls, key=lambda value: int(value, 16)),
                    "enqueue_link_calls": sorted(enqueue_link_calls, key=lambda value: int(value, 16)),
                    "lea_vtables": sorted(lea_vtables, key=lambda row: int(row["source_va"], 16)),
                    "input_variant_copies_selector_to_outparam": b"\x8b\x41\x08\x41\x89\x00" in blob[:call_offset],
                    "input_variant_loads_rcx_plus_0x10_as_payload": b"\x4c\x8b\x41\x10" in blob[:call_offset],
                    "input_variant_enqueues_to_incoming_rdx": b"\x48\x8b\xd7" in blob[call_offset : min(len(blob), call_offset + 0x20)],
                    "local_variant_loads_rcx_plus_8_payload": b"\x4c\x8b\x41\x08" in blob[:call_offset],
                    "local_variant_passes_incoming_r8_as_rdx": b"\x49\x8b\xc0" in blob[:call_offset]
                    and b"\x48\x8b\xd0" in blob[:call_offset],
                    "large_variant_uses_three_local_descriptors": len(lea_vtables) >= 3,
                    "large_variant_chains_enqueue_link": len(enqueue_link_calls) >= 1,
                    "large_variant_calls_local_parent_828cb0": target_name == "selector6_owner_variant_local_828cb0"
                    and b"\x4c\x8b\x46\x08" in call_window
                    and b"\x49\x8b\xd6" in call_window,
                    "call_window_bytes_hex": call_window.hex(),
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_continue_selector_dispatch_comparison(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    absolute_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    continue_helper_va = TARGETS["continue_entry_helper_82a270"]
    selector_helper_va = TARGETS["selector6_builder_entry_helper_82a400"]
    preflight_va = TARGETS["selector6_owner_preflight_slot_826ed0"]
    wrapper_va = TARGETS["selector6_owner_variant_wrapper_828450"]

    continue_blob = read_bytes(data, image_base, sections, continue_helper_va, 0x42)
    selector_blob = read_bytes(data, image_base, sections, selector_helper_va, 0x44)
    preflight_range = find_pdata_range_for_pc(data, image_base, sections, preflight_va)
    preflight_begin = int(str(preflight_range.get("begin_va")), 16) if preflight_range.get("begin_va") else preflight_va
    preflight_end = int(str(preflight_range.get("end_va")), 16) if preflight_range.get("end_va") else preflight_va + 0xB4
    preflight_blob = read_bytes(data, image_base, sections, preflight_begin, max(0, preflight_end - preflight_begin))
    wrapper_refs = sorted(rel32_refs.get("selector6_owner_variant_wrapper_828450", []), key=lambda row: int(str(row["source_va"]), 16))
    wrapper_caller_contexts: list[dict[str, Any]] = []
    for ref in wrapper_refs:
        source_va = int(str(ref["source_va"]), 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        begin_va = int(begin_text, 16) if begin_text else max(wrapper_va - 0x40, source_va - 0x40)
        end_va = int(end_text, 16) if end_text else source_va + 0x80
        blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
        wrapper_caller_contexts.append(
            {
                "source_va": ref["source_va"],
                "target_va": ref["target_va"],
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "checks_owner_plus_0x60_before_wrapper": b"\x80\x79\x60\x00" in blob[: max(0, source_va - begin_va)],
                "falls_back_to_selector_key_3": b"\x45\x33\xc0\x41\x8d\x50\x03" in blob,
                "fallback_calls_chain_key": any(
                    begin_va <= int(str(call_ref.get("source_va")), 16) < end_va
                    for call_ref in rel32_refs.get("selector_builder_chain_key_7a91e0", [])
                ),
                "success_path_uses_owner_plus_0x68_virtual_slot_0x10": b"\x48\x8b\x4f\x68" in blob
                and b"\xff\x50\x10" in blob,
                "passes_task_plus8_float_to_virtual_path": b"\xf3\x0f\x10\x43\x08" in blob,
                "context_bytes_hex": blob.hex(),
            }
        )

    vtable_slot_values: list[dict[str, str | None]] = []
    for slot_name, slot_va in CONTINUE_SELECTOR_COMPARISON_VTABLE_SLOTS.items():
        value = read_qword_value(data, image_base, sections, slot_va)
        vtable_slot_values.append(
            {
                "slot_name": slot_name,
                "slot_va": f"0x{slot_va:x}",
                "value_va": f"0x{value:x}" if value is not None else None,
            }
        )

    return {
        "continue_helper_va": f"0x{continue_helper_va:x}",
        "selector6_builder_helper_va": f"0x{selector_helper_va:x}",
        "continue_helper_calls_continue_entry": any(ref.get("source_va") == "0x14082a29a" for ref in rel32_refs.get("continue_entry_822b30", [])),
        "selector_builder_helper_calls_builder_entry": any(
            ref.get("source_va") == "0x14082a42d" for ref in rel32_refs.get("selector6_builder_entry_thunk_822c70", [])
        ),
        "helpers_share_task_plus8_and_stack_flag_shape": all(
            pattern in continue_blob for pattern in (b"\x4c\x8d\x41\x08", b"\x0f\xb6\x54\x24\x48", b"\x48\x8b\xcb")
        )
        and all(pattern in selector_blob for pattern in (b"\x4c\x8d\x41\x08", b"\x0f\xb6\x54\x24\x48", b"\x48\x8b\xcb")),
        "selector_helper_preserves_extra_r8_as_r9": b"\x4d\x8b\xc8" in selector_blob,
        "continue_helper_has_no_extra_r8_preserve": b"\x4d\x8b\xc8" not in continue_blob,
        "vtable_slot_values": vtable_slot_values,
        "absolute_refs": {
            "continue_entry_helper_82a270": absolute_refs.get("continue_entry_helper_82a270", []),
            "selector6_builder_entry_helper_82a400": absolute_refs.get("selector6_builder_entry_helper_82a400", []),
            "selector6_owner_preflight_slot_826ed0": absolute_refs.get("selector6_owner_preflight_slot_826ed0", []),
        },
        "preflight_range": preflight_range,
        "preflight_calls_owner_wrapper": any(ref.get("source_va") == "0x140826ef9" for ref in wrapper_refs),
        "preflight_checks_owner_plus_0x60": b"\x80\x79\x60\x00" in preflight_blob,
        "preflight_falls_back_to_selector_key_3": b"\x45\x33\xc0\x41\x8d\x50\x03" in preflight_blob
        and any(ref.get("source_va") == "0x140826f0c" for ref in rel32_refs.get("selector_builder_chain_key_7a91e0", [])),
        "preflight_success_uses_owner_plus_0x68_virtual_slot_0x10": b"\x48\x8b\x4f\x68" in preflight_blob
        and b"\xff\x50\x10" in preflight_blob,
        "preflight_passes_task_plus8_float_to_virtual_path": b"\xf3\x0f\x10\x43\x08" in preflight_blob,
        "owner_wrapper_caller_contexts": wrapper_caller_contexts,
    }


def read_selector_owner_lifecycle_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    text = next(section for section in sections if section["name"] == ".text")
    text_raw_ptr = int(text["raw_ptr"])
    text_raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[text_raw_ptr : text_raw_ptr + text_raw_size]
    vtable_lookup = {addr: name for name, addr in SELECTOR_OWNER_LIFECYCLE_VTABLE_BASES.items()}
    rip_lea_refs: list[dict[str, str]] = []
    for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05", b"\x48\x8d\x0d", b"\x48\x8d\x15"):
        index = -1
        while True:
            index = text_data.find(pattern, index + 1)
            if index < 0 or index + 7 > len(text_data):
                break
            source_va = text_va + index
            target_va = source_va + 7 + struct.unpack_from("<i", text_data, index + 3)[0]
            target_name = vtable_lookup.get(target_va)
            if target_name is None:
                continue
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            rip_lea_refs.append(
                {
                    "source_va": f"0x{source_va:x}",
                    "target_va": f"0x{target_va:x}",
                    "target_name": target_name,
                    "function_begin_va": function_range.get("begin_va"),
                    "function_end_va": function_range.get("end_va"),
                }
            )

    vtables = {
        name: {
            "base_va": f"0x{base:x}",
            "entries": read_qwords(data, image_base, sections, base, 4),
            "rip_lea_refs": sorted(
                [ref for ref in rip_lea_refs if ref.get("target_name") == name], key=lambda ref: int(str(ref["source_va"]), 16)
            ),
        }
        for name, base in SELECTOR_OWNER_LIFECYCLE_VTABLE_BASES.items()
    }

    ctor_va = TARGETS["selector_owner_ctor_821e00"]
    dtor_va = TARGETS["selector_owner_dtor_824220"]
    delete_va = TARGETS["selector_owner_delete_wrapper_826180"]
    ctor_wrapper_va = TARGETS["selector_owner_ctor_wrapper_8263c0"]
    factory_va = TARGETS["selector_owner_factory_830210"]

    ctor_range = find_pdata_range_for_pc(data, image_base, sections, ctor_va)
    dtor_range = find_pdata_range_for_pc(data, image_base, sections, dtor_va)
    delete_range = find_pdata_range_for_pc(data, image_base, sections, delete_va)
    ctor_wrapper_range = find_pdata_range_for_pc(data, image_base, sections, ctor_wrapper_va)
    factory_range = find_pdata_range_for_pc(data, image_base, sections, factory_va)

    ctor_blob = read_bytes(data, image_base, sections, ctor_va, 0xF6)
    dtor_blob = read_bytes(data, image_base, sections, dtor_va, 0xB3)
    delete_blob = read_bytes(data, image_base, sections, delete_va, 0x34)
    ctor_wrapper_blob = read_bytes(data, image_base, sections, ctor_wrapper_va, 0x107)
    factory_begin = int(str(factory_range.get("begin_va")), 16) if factory_range.get("begin_va") else factory_va
    factory_end = int(str(factory_range.get("end_va")), 16) if factory_range.get("end_va") else factory_va + 0x407
    factory_blob = read_bytes(data, image_base, sections, factory_begin, max(0, factory_end - factory_begin))

    ctor_wrapper_call_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_ctor_821e00", [])], key=lambda value: int(value, 16)
    )
    factory_main_call_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_ctor_wrapper_8263c0", [])],
        key=lambda value: int(value, 16),
    )
    factory_sibling_call_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_sibling_ctor_wrapper_826630", [])],
        key=lambda value: int(value, 16),
    )
    factory_link_sources = sorted(
        [
            str(ref.get("source_va"))
            for ref in rel32_refs.get("task_enqueue_link_7a7bb0", [])
            if factory_begin <= int(str(ref.get("source_va")), 16) < factory_end
        ],
        key=lambda value: int(value, 16),
    )
    factory_enqueue_sources = sorted(
        [
            str(ref.get("source_va"))
            for ref in rel32_refs.get("task_enqueue_7a7b60", [])
            if factory_begin <= int(str(ref.get("source_va")), 16) < factory_end
        ],
        key=lambda value: int(value, 16),
    )

    def factory_window(call_va: int) -> bytes:
        offset = call_va - factory_begin
        return factory_blob[max(0, offset - 0x28) : min(len(factory_blob), offset + 5)]

    main_call_window = factory_window(0x1408303A7)
    sibling_call_window = factory_window(0x140830319)

    return {
        "vtables": vtables,
        "constructor": {
            "function_begin_va": ctor_range.get("begin_va"),
            "function_end_va": ctor_range.get("end_va"),
            "stores_vtable_71c0": function_has_rip_lea_to(
                data, image_base, sections, ctor_va, SELECTOR_OWNER_LIFECYCLE_VTABLE_BASES["selector_owner_preflight_vtable_71c0"], 0xF6
            ),
            "allocates_0x70_bytes": b"\xba\x08\x00\x00\x00\x8d\x4a\x68" in ctor_blob,
            "copies_descriptor_to_plus_0x10": b"\x41\x8b\x06\x89\x03" in ctor_blob
            and b"\x49\x8d\x56\x10" in ctor_blob
            and b"\x49\x8b\x46\x30\x48\x89\x43\x30" in ctor_blob,
            "copies_payload_to_plus_0x50": b"\x0f\x11\x47\x50" in ctor_blob,
            "clears_plus_0x60_and_plus_0x68": b"\xc6\x47\x60\x00" in ctor_blob
            and b"\x48\xc7\x47\x68\x00\x00\x00\x00" in ctor_blob,
        },
        "destructor": {
            "function_begin_va": dtor_range.get("begin_va"),
            "function_end_va": dtor_range.get("end_va"),
            "stores_vtable_71c0": function_has_rip_lea_to(
                data, image_base, sections, dtor_va, SELECTOR_OWNER_LIFECYCLE_VTABLE_BASES["selector_owner_preflight_vtable_71c0"], 0xB3
            ),
            "releases_plus_0x68": b"\x48\x8b\x79\x68" in dtor_blob and b"\x48\x89\x73\x68" in dtor_blob,
            "cleans_vector_plus_0x20": b"\x48\x8d\x7b\x20" in dtor_blob and b"\xff\x50\x68" in dtor_blob,
        },
        "delete_wrapper": {
            "function_begin_va": delete_range.get("begin_va"),
            "function_end_va": delete_range.get("end_va"),
            "calls_destructor": rel32_refs.get("selector_owner_dtor_824220", [])
            and rel32_refs.get("selector_owner_dtor_824220", [])[0].get("source_va") == "0x14082618f",
            "frees_0x70_bytes_on_delete_flag": b"\xba\x70\x00\x00\x00" in delete_blob,
        },
        "constructor_wrapper": {
            "function_begin_va": ctor_wrapper_range.get("begin_va"),
            "function_end_va": ctor_wrapper_range.get("end_va"),
            "call_sources_to_constructor": ctor_wrapper_call_sources,
            "passes_stack_payload_and_staging_to_ctor": b"\x4c\x8d\x45\xb0" in ctor_wrapper_blob
            and b"\x48\x8d\x55\xc0" in ctor_wrapper_blob
            and b"\x48\x8b\xcb" in ctor_wrapper_blob,
            "cleans_staging_vector_after_ctor": b"\xff\x50\x68" in ctor_wrapper_blob
            and b"\x0f\x57\xc0\xf3\x0f\x7f\x45\xd8" in ctor_wrapper_blob,
        },
        "factory": {
            "function_begin_va": factory_range.get("begin_va"),
            "function_end_va": factory_range.get("end_va"),
            "calls_main_preflight_ctor_wrapper": factory_main_call_sources,
            "calls_sibling_ctor_wrapper": factory_sibling_call_sources,
            "enqueue_link_calls": factory_link_sources,
            "enqueue_calls": factory_enqueue_sources,
            "main_call_uses_r14_plus_0x18_and_incoming_r8": b"\x4d\x8b\xc7" in main_call_window
            and b"\x49\x8b\x56\x18" in main_call_window,
            "sibling_call_uses_r14_plus_0x18_and_incoming_r8": b"\x4d\x8b\xc7" in sibling_call_window
            and b"\x49\x8b\x56\x18" in sibling_call_window,
            "main_call_window_bytes_hex": main_call_window.hex(),
            "sibling_call_window_bytes_hex": sibling_call_window.hex(),
            "caller_chain": {
                "factory_called_from": sorted(
                    [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_830210", [])],
                    key=lambda value: int(value, 16),
                ),
                "factory_thunk_called_from": sorted(
                    [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_thunk_82dcb0", [])],
                    key=lambda value: int(value, 16),
                ),
                "factory_outer_called_from": sorted(
                    [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_outer_82ec20", [])],
                    key=lambda value: int(value, 16),
                ),
            },
        },
    }


def read_selector_owner_factory_entry_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    absolute_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    text = next(section for section in sections if section["name"] == ".text")
    text_raw_ptr = int(text["raw_ptr"])
    text_raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[text_raw_ptr : text_raw_ptr + text_raw_size]
    vtable_lookup = {addr: name for name, addr in SELECTOR_FACTORY_ENTRY_VTABLES.items()}
    rip_lea_refs: list[dict[str, str | None]] = []
    for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05", b"\x48\x8d\x0d", b"\x48\x8d\x15"):
        index = -1
        while True:
            index = text_data.find(pattern, index + 1)
            if index < 0 or index + 7 > len(text_data):
                break
            source_va = text_va + index
            target_va = source_va + 7 + struct.unpack_from("<i", text_data, index + 3)[0]
            target_name = vtable_lookup.get(target_va)
            if target_name is None:
                continue
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            rip_lea_refs.append(
                {
                    "source_va": f"0x{source_va:x}",
                    "target_va": f"0x{target_va:x}",
                    "target_name": target_name,
                    "function_begin_va": function_range.get("begin_va"),
                    "function_end_va": function_range.get("end_va"),
                }
            )

    vtables = {
        name: {
            "base_va": f"0x{base:x}",
            "entries": read_qwords(data, image_base, sections, base, 6),
            "rip_lea_refs": sorted(
                [ref for ref in rip_lea_refs if ref.get("target_name") == name], key=lambda ref: int(str(ref["source_va"]), 16)
            ),
        }
        for name, base in SELECTOR_FACTORY_ENTRY_VTABLES.items()
    }

    outer_caller_va = TARGETS["selector_owner_factory_outer_caller_82e350"]
    entry_helper_va = TARGETS["selector_owner_factory_entry_helper_837070"]
    neighbor_helper_va = TARGETS["selector_owner_factory_neighbor_entry_helper_837020"]
    neighbor_outer_va = TARGETS["selector_owner_factory_neighbor_outer_caller_82e310"]
    entry_vtable_base = SELECTOR_FACTORY_ENTRY_VTABLES["selector_factory_entry_vtable_b5c8"]
    neighbor_vtable_base = SELECTOR_FACTORY_ENTRY_VTABLES["selector_factory_entry_neighbor_vtable_bcc8"]

    outer_caller_range = find_pdata_range_for_pc(data, image_base, sections, outer_caller_va)
    outer_begin = int(str(outer_caller_range.get("begin_va")), 16) if outer_caller_range.get("begin_va") else outer_caller_va
    outer_end = int(str(outer_caller_range.get("end_va")), 16) if outer_caller_range.get("end_va") else outer_caller_va + 0x98
    outer_blob = read_bytes(data, image_base, sections, outer_begin, max(0, outer_end - outer_begin))

    entry_helper_range = find_pdata_range_for_pc(data, image_base, sections, entry_helper_va)
    helper_begin = int(str(entry_helper_range.get("begin_va")), 16) if entry_helper_range.get("begin_va") else entry_helper_va
    helper_end = int(str(entry_helper_range.get("end_va")), 16) if entry_helper_range.get("end_va") else entry_helper_va + 0x45
    helper_blob = read_bytes(data, image_base, sections, helper_begin, max(0, helper_end - helper_begin))

    neighbor_helper_range = find_pdata_range_for_pc(data, image_base, sections, neighbor_helper_va)
    neighbor_helper_begin = int(str(neighbor_helper_range.get("begin_va")), 16) if neighbor_helper_range.get("begin_va") else neighbor_helper_va
    neighbor_helper_end = int(str(neighbor_helper_range.get("end_va")), 16) if neighbor_helper_range.get("end_va") else neighbor_helper_va + 0x42
    neighbor_helper_blob = read_bytes(
        data, image_base, sections, neighbor_helper_begin, max(0, neighbor_helper_end - neighbor_helper_begin)
    )

    neighbor_outer_range = find_pdata_range_for_pc(data, image_base, sections, neighbor_outer_va)
    neighbor_outer_begin = int(str(neighbor_outer_range.get("begin_va")), 16) if neighbor_outer_range.get("begin_va") else neighbor_outer_va
    neighbor_outer_end = int(str(neighbor_outer_range.get("end_va")), 16) if neighbor_outer_range.get("end_va") else neighbor_outer_va + 0x39
    neighbor_outer_blob = read_bytes(data, image_base, sections, neighbor_outer_begin, max(0, neighbor_outer_end - neighbor_outer_begin))

    complex_builder_va = TARGETS["selector_factory_entry_complex_builder_8394b0"]
    complex_builder_range = find_pdata_range_for_pc(data, image_base, sections, complex_builder_va)
    complex_begin = int(str(complex_builder_range.get("begin_va")), 16) if complex_builder_range.get("begin_va") else complex_builder_va
    complex_end = int(str(complex_builder_range.get("end_va")), 16) if complex_builder_range.get("end_va") else complex_builder_va + 0xB2
    complex_blob = read_bytes(data, image_base, sections, complex_begin, max(0, complex_end - complex_begin))

    def context_for_target(target_name: str, target_va: int, size: int = 0x60, vtable_base: int | None = None) -> dict[str, Any]:
        function_range = find_pdata_range_for_pc(data, image_base, sections, target_va)
        begin_text = function_range.get("begin_va")
        begin_va = int(str(begin_text), 16) if begin_text else target_va
        blob = read_bytes(data, image_base, sections, begin_va, size)
        return {
            "target_name": target_name,
            "function_begin_va": begin_text,
            "function_end_va": function_range.get("end_va"),
            "stores_entry_vtable_b5c8": function_has_rip_lea_to(data, image_base, sections, begin_va, entry_vtable_base, size),
            "stores_neighbor_vtable_bcc8": function_has_rip_lea_to(
                data, image_base, sections, begin_va, neighbor_vtable_base if vtable_base is None else vtable_base, size
            ),
            "bytes_hex": blob.hex(),
        }

    copy_context = context_for_target("selector_owner_factory_entry_copy_835380", TARGETS["selector_owner_factory_entry_copy_835380"], 0x47)
    clone_context = context_for_target("selector_owner_factory_entry_clone_8380c0", TARGETS["selector_owner_factory_entry_clone_8380c0"], 0x47)
    dtor_context = context_for_target("selector_owner_factory_entry_dtor_8363a0", TARGETS["selector_owner_factory_entry_dtor_8363a0"], 0x38)

    return {
        "outer_caller": {
            "function_begin_va": outer_caller_range.get("begin_va"),
            "function_end_va": outer_caller_range.get("end_va"),
            "called_from": sorted(
                [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_outer_caller_82e350", [])],
                key=lambda value: int(value, 16),
            ),
            "calls_factory_outer": rel32_refs.get("selector_owner_factory_outer_82ec20", []),
            "captures_incoming_rcx_for_enqueue": b"\x48\x8b\xf9" in outer_blob and b"\x48\x8b\xd7" in outer_blob,
            "passes_incoming_r8_as_factory_rdx": b"\x49\x8b\xd0" in outer_blob,
            "passes_incoming_r9_as_factory_r8": b"\x4d\x8b\xc1" in outer_blob,
            "enqueues_factory_result_to_original_owner": any(
                outer_begin <= int(str(ref.get("source_va")), 16) < outer_end for ref in rel32_refs.get("task_enqueue_7a7b60", [])
            ),
            "bytes_hex": outer_blob.hex(),
        },
        "entry_helper": {
            "function_begin_va": entry_helper_range.get("begin_va"),
            "function_end_va": entry_helper_range.get("end_va"),
            "absolute_refs": absolute_refs.get("selector_owner_factory_entry_helper_837070", []),
            "calls_outer_caller": rel32_refs.get("selector_owner_factory_outer_caller_82e350", []),
            "passes_task_plus8_as_r8": b"\x4c\x8d\x41\x08" in helper_blob,
            "passes_stack_flag_as_edx": b"\x0f\xb6\x54\x24\x48" in helper_blob,
            "preserves_incoming_r8_as_r9": b"\x4d\x8b\xc8" in helper_blob,
            "calls_with_rbx_as_rcx": b"\x48\x8b\xcb" in helper_blob,
            "bytes_hex": helper_blob.hex(),
        },
        "neighbor_helper": {
            "function_begin_va": neighbor_helper_range.get("begin_va"),
            "function_end_va": neighbor_helper_range.get("end_va"),
            "absolute_refs": absolute_refs.get("selector_owner_factory_neighbor_entry_helper_837020", []),
            "calls_neighbor_outer": rel32_refs.get("selector_owner_factory_neighbor_outer_caller_82e310", []),
            "passes_task_plus8_as_r8": b"\x4c\x8d\x41\x08" in neighbor_helper_blob,
            "passes_stack_flag_as_edx": b"\x0f\xb6\x54\x24\x48" in neighbor_helper_blob,
            "does_not_preserve_incoming_r8_as_r9": b"\x4d\x8b\xc8" not in neighbor_helper_blob,
            "calls_with_rbx_as_rcx": b"\x48\x8b\xcb" in neighbor_helper_blob,
            "bytes_hex": neighbor_helper_blob.hex(),
        },
        "neighbor_outer": {
            "function_begin_va": neighbor_outer_range.get("begin_va"),
            "function_end_va": neighbor_outer_range.get("end_va"),
            "called_from": sorted(
                [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_neighbor_outer_caller_82e310", [])],
                key=lambda value: int(value, 16),
            ),
            "calls_neighbor_outer_thunk": rel32_refs.get("selector_owner_factory_neighbor_outer_82ebe0", []),
            "passes_incoming_r8_as_rdx": b"\x49\x8b\xd0" in neighbor_outer_blob,
            "does_not_pass_incoming_r9": b"\x4d\x8b\xc1" not in neighbor_outer_blob,
            "bytes_hex": neighbor_outer_blob.hex(),
        },
        "neighbor_chain": {
            "outer_calls_thunk": sorted(
                [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_neighbor_outer_82ebe0", [])],
                key=lambda value: int(value, 16),
            ),
            "thunk_calls_factory": sorted(
                [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_neighbor_thunk_82dc70", [])],
                key=lambda value: int(value, 16),
            ),
            "factory_called_from_thunk": sorted(
                [str(ref.get("source_va")) for ref in rel32_refs.get("selector_owner_factory_neighbor_factory_82fda0", [])],
                key=lambda value: int(value, 16),
            ),
        },
        "complex_builder_context": {
            "function_begin_va": complex_builder_range.get("begin_va"),
            "function_end_va": complex_builder_range.get("end_va"),
            "stores_entry_vtable_b5c8": function_has_rip_lea_to(data, image_base, sections, complex_begin, entry_vtable_base, 0xB2),
            "copies_two_qwords_from_rcx": b"\x48\x8b\x11\x4c\x8b\x49\x08" in complex_blob,
            "submits_with_selector_0xa": b"\x41\xb8\x0a\x00\x00\x00" in complex_blob,
            "calls_complex_submit": rel32_refs.get("selector_factory_entry_complex_submit_834b40", []),
            "bytes_hex": complex_blob.hex(),
        },
        "vtables": vtables,
        "entry_copy_context": copy_context,
        "entry_clone_context": clone_context,
        "entry_dtor_context": dtor_context,
    }


def read_selector_submit_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    submit_va = TARGETS["selector_factory_entry_complex_submit_834b40"]
    submit_range = find_pdata_range_for_pc(data, image_base, sections, submit_va)
    submit_begin = int(str(submit_range.get("begin_va")), 16) if submit_range.get("begin_va") else submit_va
    submit_end = int(str(submit_range.get("end_va")), 16) if submit_range.get("end_va") else submit_va + 0x203
    submit_blob = read_bytes(data, image_base, sections, submit_begin, max(0, submit_end - submit_begin))

    vtable_lookup = {addr: name for name, addr in SELECTOR_SUBMIT_DESCRIPTOR_VTABLES.items()}
    vtable_refs: list[dict[str, str]] = []
    for pattern in (b"\x48\x8d\x05", b"\x4c\x8d\x05", b"\x48\x8d\x0d", b"\x48\x8d\x15"):
        index = -1
        while True:
            index = submit_blob.find(pattern, index + 1)
            if index < 0 or index + 7 > len(submit_blob):
                break
            source_va = submit_begin + index
            target_va = source_va + 7 + struct.unpack_from("<i", submit_blob, index + 3)[0]
            target_name = vtable_lookup.get(target_va)
            if target_name is None:
                continue
            vtable_refs.append(
                {"source_va": f"0x{source_va:x}", "target_va": f"0x{target_va:x}", "target_name": target_name}
            )

    called_targets = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if submit_begin <= int(str(ref.get("source_va")), 16) < submit_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in [
            "selector_submit_descriptor_builder_82d840",
            "selector_submit_clone_wrapper_8347a0",
            "selector_submit_final_enqueue_7917e0",
            "selector_owner_parent_input_guard_765030",
        ]
    }
    caller_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_factory_entry_complex_submit_834b40", [])],
        key=lambda value: int(value, 16),
    )
    callsite_ranges = [find_pdata_range_for_pc(data, image_base, sections, int(source, 16)) for source in caller_sources]

    return {
        "function_begin_va": submit_range.get("begin_va"),
        "function_end_va": submit_range.get("end_va"),
        "caller_sources": caller_sources,
        "caller_function_ranges": callsite_ranges,
        "called_targets": called_targets,
        "captures_incoming_rcx_rdx_r8_r9": b"\x4d\x8b\xf1" in submit_blob
        and b"\x48\x8b\xf2" in submit_blob
        and b"\x48\x8b\xf9" in submit_blob
        and b"\x44\x89\x45\x58" in submit_blob,
        "reads_stack_arg_0x130_into_rbx": b"\x48\x8b\x9d\x30\x01\x00\x00" in submit_blob,
        "reads_owner_plus_0x38_source": b"\x48\x8b\x4b\x38" in submit_blob,
        "calls_owner_plus0_virtual_builder": b"\x48\x8b\x01" in submit_blob and b"\xff\x10" in submit_blob,
        "builds_descriptor_vtable_bde0": any(ref.get("target_name") == "selector_submit_descriptor_vtable_bde0" for ref in vtable_refs),
        "copies_incoming_payload_into_descriptor": b"\x48\x89\x74\x24\x38" in submit_blob
        and b"\x4c\x89\x74\x24\x40" in submit_blob
        and b"\x0f\x10\x44\x24\x38" in submit_blob,
        "stores_selector_argument_r8d": b"\x44\x89\x45\x58" in submit_blob and b"\x8b\x45\x58" in submit_blob,
        "passes_descriptor_builder_into_clone_wrapper": b"\x48\x8d\x55\xb8" in submit_blob
        and b"\x48\x8d\x4d\x08" in submit_blob
        and b"\x48\x8b\xd0" in submit_blob
        and b"\x48\x8d\x8d\xa0\x00\x00\x00" in submit_blob,
        "final_enqueue_uses_original_rcx_owner_and_descriptor": b"\x4c\x8d\x44\x24\x48" in submit_blob
        and b"\x48\x8b\xd7" in submit_blob
        and b"\x48\x8b\xc8" in submit_blob,
        "cleans_temporary_owned_objects": b"\xff\x50\x20" in submit_blob
        and b"\x4c\x89\x7b\x38" in submit_blob,
        "returns_original_owner_rdi": b"\x48\x8b\xc7" in submit_blob,
        "vtable_refs": vtable_refs,
        "bytes_hex": submit_blob.hex(),
    }


def read_selector_final_enqueue_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    final_va = TARGETS["selector_submit_final_enqueue_7917e0"]
    final_range = find_pdata_range_for_pc(data, image_base, sections, final_va)
    final_begin = int(str(final_range.get("begin_va")), 16) if final_range.get("begin_va") else final_va
    final_end = int(str(final_range.get("end_va")), 16) if final_range.get("end_va") else final_va + 0x11A
    final_blob = read_bytes(data, image_base, sections, final_begin, max(0, final_end - final_begin))
    caller_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_submit_final_enqueue_7917e0", [])],
        key=lambda value: int(value, 16),
    )
    caller_contexts: list[dict[str, Any]] = []
    for source_text in caller_sources:
        source_va = int(source_text, 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        begin_va = int(str(begin_text), 16) if begin_text else source_va - 0x80
        end_va = int(str(end_text), 16) if end_text else source_va + 0x40
        blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
        call_offset = source_va - begin_va
        call_window = blob[max(0, call_offset - 0x40) : min(len(blob), call_offset + 5)]
        caller_contexts.append(
            {
                "source_va": source_text,
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "menu_region": bool(begin_text and 0x140820000 <= int(str(begin_text), 16) < 0x140840000),
                "is_selector_submit_callsite": source_text == "0x140834c8f" and begin_text == "0x140834b40",
                "is_entry_family_callsite": source_text == "0x140827900" and begin_text == "0x1408277d0",
                "is_selector_builder_callsite": source_text == "0x14082ad64"
                and begin_text == "0x14082ac70"
                and function_has_rip_lea_to(data, image_base, sections, begin_va, 0x142AC7610, max(0, end_va - begin_va)),
                "is_global_runtime_callsite": source_text == "0x14080da8b"
                and begin_text == "0x14080d990"
                and b"\x48\x8b\xb0\x80\x00\x00\x00" in blob,
                "is_outside_menu_runtime_callsite": source_text == "0x1409ac6fc" and begin_text == "0x1409ac620",
                "call_window_uses_descriptor_output_rdx": b"\x48\x8d\x54\x24" in call_window,
                "call_window_uses_saved_output_rdi_as_rdx": b"\x48\x8b\xd7" in call_window,
                "call_window_uses_stack_descriptor_r8": b"\x4c\x8d\x44\x24" in call_window,
                "call_window_uses_rax_as_rcx": b"\x48\x8b\xc8" in call_window,
                "call_window_bytes_hex": call_window.hex(),
            }
        )
    called_targets = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if final_begin <= int(str(ref.get("source_va")), 16) < final_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in ["selector_final_enqueue_pair_builder_790fa0"]
    }
    return {
        "function_begin_va": final_range.get("begin_va"),
        "function_end_va": final_range.get("end_va"),
        "caller_sources": caller_sources,
        "caller_contexts": caller_contexts,
        "called_targets": called_targets,
        "captures_incoming_rcx_rdx_r8": b"\x49\x8b\xd8" in final_blob
        and b"\x48\x8b\xfa" in final_blob
        and b"\x48\x8b\xe9" in final_blob,
        "allocates_0xa0_pair_object": b"\x41\x8d\x56\x08" in final_blob
        and b"\xb9\xa0\x00\x00\x00" in final_blob,
        "copies_r8_plus38_source_into_stack48": b"\x48\x8b\x4b\x38" in final_blob
        and b"\x48\x8d\x54\x24\x48" in final_blob
        and b"\xff\x10" in final_blob,
        "copies_rcx_plus38_source_into_stack88": b"\x48\x8b\x4d\x38" in final_blob
        and b"\x48\x8d\x94\x24\x88\x00\x00\x00" in final_blob,
        "calls_pair_builder_with_stack_descriptors": called_targets.get("selector_final_enqueue_pair_builder_790fa0") == ["0x1407918ab"],
        "stores_pair_result_to_output_rdx": b"\x48\x89\x07" in final_blob,
        "increments_ref_if_result_nonnull": b"\x48\x8d\x48\x08" in final_blob
        and b"\xe8\xfa\x88\x72\x01" in final_blob,
        "cleans_r8_plus38_after_pair_build": b"\x48\x8b\x4b\x38" in final_blob
        and b"\x48\x3b\xcb" in final_blob
        and b"\xff\x50\x20" in final_blob
        and b"\x4c\x89\x73\x38" in final_blob,
        "returns_output_pointer_rdi": b"\x48\x8b\xc7" in final_blob,
        "bytes_hex": final_blob.hex(),
    }


def read_selector_pair_builder_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    pair_va = TARGETS["selector_final_enqueue_pair_builder_790fa0"]
    pair_range = find_pdata_range_for_pc(data, image_base, sections, pair_va)
    pair_begin = int(str(pair_range.get("begin_va")), 16) if pair_range.get("begin_va") else pair_va
    pair_end = int(str(pair_range.get("end_va")), 16) if pair_range.get("end_va") else pair_va + 0xCA
    pair_blob = read_bytes(data, image_base, sections, pair_begin, max(0, pair_end - pair_begin))
    caller_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("selector_final_enqueue_pair_builder_790fa0", [])],
        key=lambda value: int(value, 16),
    )
    return {
        "function_begin_va": pair_range.get("begin_va"),
        "function_end_va": pair_range.get("end_va"),
        "caller_sources": caller_sources,
        "captures_output_rcx_and_sources_rdx_r8": b"\x49\x8b\xf8" in pair_blob
        and b"\x48\x8b\xea" in pair_blob
        and b"\x48\x8b\xd9" in pair_blob,
        "calls_header_init_before_vtable_store": b"\xe8\x50\x5f\x01\x00" in pair_blob
        and b"\x48\x8d\x05\x18\x17\x31\x02" in pair_blob,
        "stores_pair_vtable_aa26f0": function_has_rip_lea_to(data, image_base, sections, pair_begin, 0x142AA26F0, max(0, pair_end - pair_begin))
        and b"\x48\x89\x03" in pair_blob,
        "clears_output_slots_10_18": b"\x44\x89\x73\x10" in pair_blob and b"\x4c\x89\x73\x18" in pair_blob,
        "clones_left_source_plus38_to_output_plus20": b"\x48\x8d\x73\x20" in pair_blob
        and b"\x48\x8b\x4d\x38" in pair_blob
        and b"\x48\x89\x46\x38" in pair_blob,
        "clones_right_source_plus38_to_output_plus60": b"\x48\x8d\x73\x60" in pair_blob
        and b"\x48\x8b\x4f\x38" in pair_blob
        and b"\x48\x89\x46\x38" in pair_blob,
        "cleans_left_source_after_clone": b"\x48\x8b\x4d\x38" in pair_blob
        and b"\x48\x3b\xcd" in pair_blob
        and b"\xff\x50\x20" in pair_blob
        and b"\x4c\x89\x75\x38" in pair_blob,
        "cleans_right_source_after_clone": b"\x48\x8b\x4f\x38" in pair_blob
        and b"\x48\x3b\xcf" in pair_blob
        and b"\xff\x50\x20" in pair_blob
        and b"\x4c\x89\x77\x38" in pair_blob,
        "returns_output_rbx": b"\x48\x8b\xc3" in pair_blob,
        "bytes_hex": pair_blob.hex(),
    }


def read_set_save_slot_callsite_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    contexts: list[dict[str, Any]] = []
    for ref in sorted(
        rel32_refs.get("set_save_slot_67a810", []),
        key=lambda item: int(str(item.get("source_va")), 16),
    ):
        source_text = str(ref.get("source_va"))
        source_va = int(source_text, 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        begin_va = int(str(begin_text), 16) if begin_text else source_va - 0x80
        end_va = int(str(end_text), 16) if end_text else source_va + 0x40
        blob = read_bytes(data, image_base, sections, begin_va, max(0, end_va - begin_va))
        call_offset = source_va - begin_va
        call_window = blob[max(0, call_offset - 0x40) : min(len(blob), call_offset + 5)]
        contexts.append(
            {
                "source_va": source_text,
                "return_va": f"0x{source_va + 5:x}",
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "menu_region": bool(begin_text and 0x140820000 <= int(str(begin_text), 16) < 0x140840000),
                "passes_minus_one_in_ecx": b"\x83\xc9\xff" in call_window
                or b"\xb9\xff\xff\xff\xff" in call_window,
                "increments_owner_plus_b0_before_call": b"\xff\x81\xb0\x00\x00\x00" in blob,
                "checks_owner_plus_b0_before_call": b"\x83\xb9\xb0\x00\x00\x00\x01" in blob,
                "stores_owner_plus_4c_minus_one_after_call": b"\xc7\x43\x4c\xff\xff\xff\xff" in blob,
                "loads_global_after_call": b"\x48\x8b\x0d" in blob[max(0, call_offset) : min(len(blob), call_offset + 0x20)],
                "call_window_bytes_hex": call_window.hex(),
            }
        )
    return contexts


def read_game_man_b5e_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    direct_patterns = [
        ("read_u8_rax_plus_b5e", b"\x0f\xb6\x80\x5e\x0b\x00\x00"),
        ("write_imm8_zero_rax_plus_b5e", b"\xc6\x80\x5e\x0b\x00\x00\x00"),
        ("write_cl_rax_plus_b5e", b"\x88\x88\x5e\x0b\x00\x00"),
    ]
    direct_accesses: list[dict[str, Any]] = []
    for access_kind, needle in direct_patterns:
        cursor = 0
        while True:
            index = text_data.find(needle, cursor)
            if index < 0:
                break
            source_va = text_va + index
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            direct_accesses.append(
                {
                    "source_va": f"0x{source_va:x}",
                    "source_rva": f"0x{source_va - image_base:x}",
                    "access_kind": access_kind,
                    "function_begin_va": function_range.get("begin_va"),
                    "function_end_va": function_range.get("end_va"),
                    "bytes_hex": needle.hex(),
                }
            )
            cursor = index + 1
    by_target = {addr: name for name, addr in TARGETS.items()}

    def call_contexts(target_name: str) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for ref in sorted(rel32_refs.get(target_name, []), key=lambda item: int(str(item.get("source_va")), 16)):
            source_va = int(str(ref.get("source_va")), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            window_start = max(text_va, source_va - 0x80)
            window_end = min(text_va + len(text_data), source_va + 0x120)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            if begin_text is not None:
                window_start = max(window_start, int(str(begin_text), 16))
            if end_text is not None:
                window_end = min(window_end, int(str(end_text), 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            call_offset = source_va - window_start
            local_calls: list[dict[str, Any]] = []
            for index in range(max(0, len(blob) - 5)):
                if blob[index] not in (0xE8, 0xE9):
                    continue
                call_source_va = window_start + index
                target_va = call_source_va + 5 + struct.unpack_from("<i", blob, index + 1)[0]
                local_calls.append(
                    {
                        "kind": "call" if blob[index] == 0xE8 else "jmp",
                        "source_va": f"0x{call_source_va:x}",
                        "target_va": f"0x{target_va:x}",
                        "target_name": by_target.get(target_va),
                    }
                )
            before = blob[max(0, call_offset - 0x20) : call_offset]
            after = blob[call_offset + 5 : min(len(blob), call_offset + 0x60)]
            rows.append(
                {
                    "source_va": str(ref.get("source_va")),
                    "target_name": target_name,
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "move_map_region": 0x140AF0000 <= source_va < 0x140B00000,
                    "title_step_region": 0x140B00000 <= source_va < 0x140B20000,
                    "passes_zero_to_setter": target_name == "slot_reset_end_flow_reset_67ae90"
                    and (b"\x33\xc9" in before or b"\xb1\x00" in before),
                    "passes_one_to_setter": target_name == "slot_reset_end_flow_reset_67ae90" and b"\xb1\x01" in before,
                    "branches_on_getter_result": target_name == "game_man_b5e_getter_67a1b0"
                    and b"\x84\xc0" in after
                    and (b"\x74" in after[:8] or b"\x75" in after[:8] or b"\x0f\x84" in after[:16] or b"\x0f\x85" in after[:16]),
                    "cmov_uses_getter_result": target_name == "game_man_b5e_getter_67a1b0"
                    and (b"\x0f\x45" in after or b"\x0f\x44" in after),
                    "nearby_request_save": any(call.get("target_name") == "request_save_67a520" for call in local_calls),
                    "nearby_save_request_profile": any(
                        call.get("target_name") == "save_request_profile_67a420" for call in local_calls
                    ),
                    "nearby_set_save_slot_wrapper_67a820": any(
                        call.get("target_va") == "0x14067a820" for call in local_calls
                    ),
                    "local_calls": local_calls,
                    "call_window_bytes_hex": blob.hex(),
                }
            )
        return rows

    getter_call_contexts = call_contexts("game_man_b5e_getter_67a1b0")
    setter_call_contexts = call_contexts("slot_reset_end_flow_reset_67ae90")
    bulk_clear_call_contexts = call_contexts("game_man_b5e_bulk_clear_679830")
    return {
        "direct_accesses": sorted(direct_accesses, key=lambda item: int(str(item["source_va"]), 16)),
        "getter_call_contexts": getter_call_contexts,
        "setter_call_contexts": setter_call_contexts,
        "bulk_clear_call_contexts": bulk_clear_call_contexts,
        "getter_call_sources": [row["source_va"] for row in getter_call_contexts],
        "setter_call_sources": [row["source_va"] for row in setter_call_contexts],
        "bulk_clear_call_sources": [row["source_va"] for row in bulk_clear_call_contexts],
    }


def read_utf16z(data: bytes, image_base: int, sections: list[dict[str, int | str]], va: int, max_bytes: int = 160) -> str:
    blob = read_bytes(data, image_base, sections, va, max_bytes)
    end = 0
    while end + 1 < len(blob):
        if blob[end] == 0 and blob[end + 1] == 0:
            break
        end += 2
    if end == 0:
        return ""
    try:
        return blob[:end].decode("utf-16le", errors="replace")
    except Exception:
        return ""


def read_slot_reset_state_table_init_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
) -> dict[str, Any]:
    init_va = TARGETS["slot_reset_state_table_init_0a4f50"]
    table_va = TARGETS["slot_reset_state_table_global_43d71580"]
    init_range = find_pdata_range_for_pc(data, image_base, sections, init_va)
    init_begin = int(str(init_range.get("begin_va")), 16) if init_range.get("begin_va") else init_va
    init_end = int(str(init_range.get("end_va")), 16) if init_range.get("end_va") else init_va + 0x16D
    blob = read_bytes(data, image_base, sections, init_begin, max(0, init_end - init_begin))
    stores: list[dict[str, Any]] = []
    for index in range(max(0, len(blob) - 14)):
        if blob[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
            continue
        if blob[index + 7 : index + 10] != b"\x48\x89\x05":
            continue
        value_disp = struct.unpack_from("<i", blob, index + 3)[0]
        store_disp = struct.unpack_from("<i", blob, index + 10)[0]
        source_va = init_begin + index
        value_va = source_va + 7 + value_disp
        store_va = source_va + 14 + store_disp
        if not (table_va <= store_va < table_va + 0xD0):
            continue
        offset = store_va - table_va
        entry_index = offset // 16
        entry_slot = "handler" if offset % 16 == 0 else "label"
        row: dict[str, Any] = {
            "source_va": f"0x{source_va:x}",
            "store_va": f"0x{store_va:x}",
            "store_offset": f"0x{offset:x}",
            "entry_index": entry_index,
            "entry_slot": entry_slot,
            "value_va": f"0x{value_va:x}",
        }
        if entry_slot == "label":
            row["label_text"] = read_utf16z(data, image_base, sections, value_va)
        stores.append(row)
    entries: list[dict[str, Any]] = []
    for entry_index in sorted({int(row["entry_index"]) for row in stores}):
        handler = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "handler"), None)
        label = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "label"), None)
        entries.append(
            {
                "entry_index": entry_index,
                "handler_va": handler.get("value_va") if handler else None,
                "handler_source_va": handler.get("source_va") if handler else None,
                "label_va": label.get("value_va") if label else None,
                "label_source_va": label.get("source_va") if label else None,
                "label_text": label.get("label_text") if label else None,
            }
        )
    return {
        "function_begin_va": init_range.get("begin_va"),
        "function_end_va": init_range.get("end_va"),
        "table_base_va": f"0x{table_va:x}",
        "zeroes_table_base": function_has_rip_lea_any_reg_to(data, image_base, sections, init_begin, table_va, max(0, init_end - init_begin)),
        "zero_size_bytes": 0xD0 if b"\x41\xb8\xd0\x00\x00\x00" in blob else None,
        "stores": stores,
        "entries": entries,
        "bytes_hex": blob.hex(),
    }


def read_slot_reset_global_toggle_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    helper_va = TARGETS["slot_reset_menu_job_wait_global_toggle_7663c0"]
    helper_blob = read_bytes(data, image_base, sections, helper_va, 4)
    callers = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_menu_job_wait_global_toggle_7663c0", [])],
        key=lambda value: int(value, 16),
    )

    caller_windows: dict[str, dict[str, Any]] = {}
    for source in callers:
        source_va = int(source, 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        function_begin = int(str(function_range.get("begin_va")), 16) if function_range.get("begin_va") else source_va
        function_end = int(str(function_range.get("end_va")), 16) if function_range.get("end_va") else source_va + 1
        window_begin = max(function_begin, source_va - 0x30)
        window_end = min(function_end, source_va + 0x20)
        window = read_bytes(data, image_base, sections, window_begin, max(0, window_end - window_begin))
        caller_windows[source] = {
            "function_begin_va": function_range.get("begin_va"),
            "function_end_va": function_range.get("end_va"),
            "window_begin_va": f"0x{window_begin:x}",
            "window_end_va": f"0x{window_end:x}",
            "loads_global_job_context_before_call": b"\x48\x8b\x0d" in window,
            "passes_true_in_dl": b"\xb2\x01" in window,
            "window_bytes_hex": window.hex(),
        }

    extra_parent_begin = None
    extra_parent_end = None
    extra_parent_blob = b""
    extra_first_call = caller_windows.get("0x140b01daf", {})
    if extra_first_call.get("function_begin_va") and extra_first_call.get("function_end_va"):
        extra_parent_begin = int(str(extra_first_call["function_begin_va"]), 16)
        extra_parent_end = int(str(extra_first_call["function_end_va"]), 16)
        extra_parent_blob = read_bytes(
            data, image_base, sections, extra_parent_begin, max(0, extra_parent_end - extra_parent_begin)
        )
    extra_parent_target_names = [
        "slot_reset_global_toggle_extra_gate_a9cd00",
        "slot_reset_global_toggle_extra_toggle_gate_a9cc90",
        "slot_reset_global_toggle_extra_item_a9cdb0",
        "slot_reset_global_toggle_extra_status_a9c9d0",
        "slot_reset_global_toggle_extra_status_a9cd30",
        "slot_reset_global_toggle_extra_probe_e2a5c0",
        "slot_reset_global_toggle_extra_probe_e2a5e0",
        "slot_reset_global_toggle_extra_probe_e29bc0",
        "slot_reset_global_toggle_extra_probe_e29930",
        "slot_reset_global_toggle_extra_callback_7edf40",
        "slot_reset_global_toggle_extra_callback_7edf90",
    ]
    extra_parent_calls = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if extra_parent_begin is not None
                and extra_parent_end is not None
                and extra_parent_begin <= int(str(ref.get("source_va")), 16) < extra_parent_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in extra_parent_target_names
    }
    global_context_refs = scan_rip_relative_refs_to_va(
        data, image_base, sections, TARGETS["slot_reset_global_job_context_43d6b7b0"]
    )
    global_context_functions: dict[str, dict[str, Any]] = {}
    for ref in global_context_refs:
        begin_text = ref.get("function_begin_va")
        end_text = ref.get("function_end_va")
        if not begin_text or not end_text or str(begin_text) in global_context_functions:
            continue
        function_begin = int(str(begin_text), 16)
        function_end = int(str(end_text), 16)
        blob = read_bytes(data, image_base, sections, function_begin, max(0, function_end - function_begin))
        plus19_cmp_count = sum(
            blob.count(pattern)
            for pattern in [
                b"\x80\x78\x19",
                b"\x80\x79\x19",
                b"\x80\x7a\x19",
                b"\x80\x7b\x19",
                b"\x80\x7e\x19",
                b"\x80\x7f\x19",
                b"\x80\xb8\x19\x00\x00\x00",
            ]
        )
        global_context_functions[str(begin_text)] = {
            "function_begin_va": begin_text,
            "function_end_va": end_text,
            "ref_sources": [
                str(other_ref.get("source_va"))
                for other_ref in global_context_refs
                if other_ref.get("function_begin_va") == begin_text
            ],
            "writes_plus18_direct": b"\xc6\x40\x18" in blob or b"\xc6\x80\x18\x00\x00\x00" in blob,
            "writes_plus19_direct": b"\xc6\x40\x19" in blob or b"\xc6\x80\x19\x00\x00\x00" in blob,
            "reads_plus18_direct": b"\x80\x78\x18" in blob or b"\x80\xb8\x18\x00\x00\x00" in blob,
            "reads_plus19_direct": plus19_cmp_count > 0,
            "plus19_cmp_count": plus19_cmp_count,
            "node_plus18_mark_count": sum(
                blob.count(pattern) for pattern in [b"\xc6\x47\x18\x01", b"\xc6\x42\x18\x01"]
            ),
        }
    plus19_reader_functions = sorted(
        [key for key, value in global_context_functions.items() if value.get("reads_plus19_direct")],
        key=lambda value: int(value, 16),
    )
    plus19_gate_begin = TARGETS["slot_reset_global_context_plus19_gate_758050"]
    plus19_gate_end = 0x140758199
    plus19_gate_blob = read_bytes(data, image_base, sections, plus19_gate_begin, plus19_gate_end - plus19_gate_begin)
    plus19_gate_callers = sorted(
        [
            str(ref.get("source_va"))
            for ref in rel32_refs.get("slot_reset_global_context_plus19_gate_758050", [])
        ],
        key=lambda value: int(value, 16),
    )
    plus19_gate_status_calls = sorted(
        [
            str(ref.get("source_va"))
            for ref in rel32_refs.get("slot_reset_global_context_plus19_gate_status_b3d310", [])
            if plus19_gate_begin <= int(str(ref.get("source_va")), 16) < plus19_gate_end
        ],
        key=lambda value: int(value, 16),
    )

    return {
        "helper_va": f"0x{helper_va:x}",
        "helper_bytes_hex": helper_blob.hex(),
        "stores_dl_to_context_plus_18": helper_blob == b"\x88\x51\x18\xc3",
        "callers": callers,
        "caller_windows": caller_windows,
        "ending_menujobwait_call": "0x140ae5432" if "0x140ae5432" in callers else None,
        "title_menujobwait_call": "0x140b0d4e9" if "0x140b0d4e9" in callers else None,
        "extra_parent_begin_va": f"0x{extra_parent_begin:x}" if extra_parent_begin is not None else None,
        "extra_parent_end_va": f"0x{extra_parent_end:x}" if extra_parent_end is not None else None,
        "extra_parent_callers": [source for source in callers if source in {"0x140b01daf", "0x140b01f90"}],
        "extra_parent_calls_by_target": extra_parent_calls,
        "extra_parent_clears_context_plus19_before_first_toggle": b"\xc6\x40\x19\x00" in extra_parent_blob,
        "extra_parent_first_call_has_false_and_true_paths": b"\x33\xd2\xeb\x35" in extra_parent_blob
        and b"\xb2\x01\xe8" in extra_parent_blob,
        "extra_parent_first_toggle_follows_a9cc90_gate": extra_parent_calls.get(
            "slot_reset_global_toggle_extra_toggle_gate_a9cc90"
        )
        == ["0x140b01d33"],
        "extra_parent_second_call_sets_context_plus19_then_true": b"\xc6\x40\x19\x01" in extra_parent_blob
        and b"\xb2\x01\xe8" in extra_parent_blob,
        "extra_parent_second_toggle_follows_failed_a9cd00_gate": extra_parent_calls.get(
            "slot_reset_global_toggle_extra_gate_a9cd00"
        )
        == ["0x140b01cae"],
        "extra_parent_first_toggle_callback_chain_mapped": extra_parent_calls.get(
            "slot_reset_global_toggle_extra_probe_e2a5c0"
        )
        == ["0x140b01db8"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_item_a9cdb0") == ["0x140b01e0d"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_status_a9c9d0") == ["0x140b01e5f"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_status_a9cd30") == ["0x140b01eb4"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_probe_e2a5e0") == ["0x140b01ec0"],
        "extra_parent_second_toggle_callback_chain_mapped": extra_parent_calls.get(
            "slot_reset_global_toggle_extra_probe_e29bc0"
        )
        == ["0x140b01f99"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_callback_7edf40") == ["0x140b01fe8"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_probe_e29930") == ["0x140b01ff1"]
        and extra_parent_calls.get("slot_reset_global_toggle_extra_callback_7edf90") == ["0x140b02040"],
        "global_context_ref_count": len(global_context_refs),
        "global_context_ref_sources": [str(ref.get("source_va")) for ref in global_context_refs],
        "global_context_functions": global_context_functions,
        "global_context_plus19_reader_functions": plus19_reader_functions,
        "global_context_plus19_gate_context": {
            "function_begin_va": f"0x{plus19_gate_begin:x}",
            "function_end_va": f"0x{plus19_gate_end:x}",
            "callers": plus19_gate_callers,
            "caller_count": len(plus19_gate_callers),
            "status_calls": plus19_gate_status_calls,
            "loads_global_context_before_plus1a_check": b"\x48\x8b\x05" in plus19_gate_blob
            and b"\x38\x58\x1a" in plus19_gate_blob,
            "queries_status_when_plus1a_differs": plus19_gate_status_calls == ["0x1407580f0"],
            "requires_input_byte_before_plus19_gate": b"\x80\x3e\x00" in plus19_gate_blob
            and b"\x74\x59" in plus19_gate_blob,
            "requires_context_798_empty_and_plus19_true": b"\x48\x83\xb8\x98\x07\x00\x00\x00" in plus19_gate_blob
            and b"\x80\x78\x19\x00" in plus19_gate_blob,
            "also_requires_status_result_nonzero": b"\x85\xff" in plus19_gate_blob,
            "bytes_hex": plus19_gate_blob.hex(),
        },
        "global_context_known_toggle_functions": {
            key: global_context_functions.get(key, {})
            for key in ["0x140ae5390", "0x140b01be0", "0x140b0d400"]
        },
        "extra_parent_bytes_hex": extra_parent_blob.hex(),
    }


def read_slot_reset_timed_queue_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    submit_va = TARGETS["slot_reset_menu_job_wait_task_submit_733f20"]
    queue_va = TARGETS["slot_reset_menu_job_wait_queue_7a9600"]
    check_va = TARGETS["slot_reset_menu_job_wait_queue_check_7a9200"]
    submit_range = find_pdata_range_for_pc(data, image_base, sections, submit_va)
    queue_range = find_pdata_range_for_pc(data, image_base, sections, queue_va)
    submit_begin = int(str(submit_range.get("begin_va")), 16) if submit_range.get("begin_va") else submit_va
    submit_end = int(str(submit_range.get("end_va")), 16) if submit_range.get("end_va") else submit_va + 0xC0
    queue_begin = int(str(queue_range.get("begin_va")), 16) if queue_range.get("begin_va") else queue_va
    queue_end = int(str(queue_range.get("end_va")), 16) if queue_range.get("end_va") else queue_va + 0x131
    submit_blob = read_bytes(data, image_base, sections, submit_begin, max(0, submit_end - submit_begin))
    queue_blob = read_bytes(data, image_base, sections, queue_begin, max(0, queue_end - queue_begin))
    check_blob = read_bytes(data, image_base, sections, check_va, 7)
    timed_base_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["slot_reset_timed_descriptor_base_vtable_29c8e48"], 2
    )
    timed_active_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["slot_reset_timed_descriptor_active_vtable_29c8e58"], 2
    )
    submit_calls = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if submit_begin <= int(str(ref.get("source_va")), 16) < submit_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in ["task_enqueue_7a7b60", "task_enqueue_link_7a7bb0"]
    }
    queue_calls = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if queue_begin <= int(str(ref.get("source_va")), 16) < queue_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in ["slot_reset_menu_job_wait_queue_check_7a9200"]
    }
    return {
        "submit_function_begin_va": submit_range.get("begin_va"),
        "submit_function_end_va": submit_range.get("end_va"),
        "queue_function_begin_va": queue_range.get("begin_va"),
        "queue_function_end_va": queue_range.get("end_va"),
        "queue_check_function_begin_va": f"0x{check_va:x}",
        "queue_check_function_end_va": f"0x{check_va + len(check_blob):x}",
        "timed_base_vtable_slots": timed_base_vtable_slots,
        "timed_active_vtable_slots": timed_active_vtable_slots,
        "submit_calls_by_target": submit_calls,
        "queue_calls_by_target": queue_calls,
        "submit_reads_owner_48_count": b"\x48\x8b\x41\x48" in submit_blob,
        "submit_empty_returns_false_descriptor": b"\x48\x85\xc0\x75\x18" in submit_blob
        and b"\x48\x8d\x05\x08\x4f\x29\x02" in submit_blob
        and b"\x48\x89\x02" in submit_blob
        and b"\x32\xc0" in submit_blob,
        "submit_iterates_entries_backwards": b"\x48\x63\xf9\x78\x2e" in submit_blob
        and b"\x48\x8d\x1c\xfb" in submit_blob
        and b"\x48\x03\xdd" in submit_blob
        and b"\x48\x8d\x5b\xf8" in submit_blob
        and b"\x48\x83\xef\x01" in submit_blob
        and b"\x79\xe3" in submit_blob,
        "submit_calls_entry_virtual_slot10_with_task_time": b"\xf3\x0f\x10\x4e\x08" in submit_blob
        and b"\xff\x50\x10" in submit_blob,
        "submit_returns_true_descriptor_after_iteration": b"\xb0\x01" in submit_blob
        and b"\x48\x89\x06" in submit_blob,
        "timed_descriptor_vtable_pair_mapped": [row["value_va"] for row in timed_base_vtable_slots]
        == ["0x1401205c0", "0x1432ab380"]
        and [row["value_va"] for row in timed_active_vtable_slots]
        == ["0x140120590", "0x1432ab478"],
        "submit_restores_timed_descriptor_vtables_on_empty_and_after_iteration": b"\x48\x8d\x05\x08\x4f\x29\x02\x48\x89\x02\x48\x8d\x05\xee\x4e\x29\x02\x48\x89\x02" in submit_blob
        and b"\x48\x8d\x05\x94\x4e\x29\x02\x48\x89\x06\x48\x8d\x05\x7a\x4e\x29\x02\x48\x89\x06" in submit_blob,
        "submit_is_pump_only_no_new_child_enqueue": b"\xff\x50\x10" in submit_blob
        and submit_calls.get("task_enqueue_7a7b60") == []
        and submit_calls.get("task_enqueue_link_7a7bb0") == [],
        "queue_captures_slot_and_descriptor": b"\x48\x8b\xf2" in queue_blob
        and b"\x4c\x8b\xf1" in queue_blob
        and b"\x48\x8b\x19" in queue_blob,
        "queue_builds_timed_descriptor_from_arg_plus8": b"\xf3\x0f\x10\x46\x08" in queue_blob,
        "queue_invokes_existing_job_virtual_slot10": b"\xff\x50\x10" in queue_blob,
        "queue_checks_completion_with_7a9200": queue_calls.get("slot_reset_menu_job_wait_queue_check_7a9200") == [
            "0x1407a9696"
        ],
        "queue_check_returns_descriptor_status_gt_one": check_blob == b"\x83\x39\x01\x0f\x97\xc0\xc3",
        "queue_passes_virtual_result_descriptor_to_check": b"\x48\x8d\x8c\x24\x80\x00\x00\x00\xe8\x65\xfb\xff\xff" in queue_blob,
        "queue_terminal_check_gates_current_slot_clear": b"\x84\xc0\x74\x43\x49\x8b\x3e\x48\x3b\xfb" in queue_blob,
        "queue_clears_slot_when_existing_job_consumed": b"\x49\x8b\x3e" in queue_blob
        and b"\x48\x3b\xfb" in queue_blob
        and b"\x49\xc7\x06\x00\x00\x00\x00" in queue_blob,
        "queue_releases_existing_job_and_clears_temp_descriptor": b"\x48\xc7\x84\x24\x90\x00\x00\x00\x00\x00\x00\x00" in queue_blob
        and b"\x4c\x89\x3e" in queue_blob
        and b"\x4c\x89\x26" in queue_blob,
        "queue_restores_incoming_descriptor_without_creating_job_when_slot_empty": b"\x48\x85\xdb\x0f\x84\xe2\x00\x00\x00" in queue_blob
        and b"\x4c\x89\x3e\x4c\x89\x26" in queue_blob,
        "queue_is_single_slot_consumer_not_enqueue": b"\x48\x8b\x19" in queue_blob
        and b"\xff\x50\x10" in queue_blob
        and b"\x49\xc7\x06\x00\x00\x00\x00" in queue_blob,
        "submit_bytes_hex": submit_blob.hex(),
        "queue_bytes_hex": queue_blob.hex(),
        "queue_check_bytes_hex": check_blob.hex(),
    }


def read_title_accept_payload_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    update_va = 0x1409B26D8
    update_range = find_pdata_range_for_pc(data, image_base, sections, update_va)
    update_begin = int(str(update_range.get("begin_va")), 16) if update_range.get("begin_va") else 0x1409B24E0
    update_end = int(str(update_range.get("end_va")), 16) if update_range.get("end_va") else 0x1409B2CDB
    update_blob = read_bytes(data, image_base, sections, update_begin, max(0, update_end - update_begin))

    def range_blob(target_name: str, fallback_end: int) -> tuple[dict[str, Any], int, int, bytes]:
        va = TARGETS[target_name]
        function_range = find_pdata_range_for_pc(data, image_base, sections, va)
        begin = int(str(function_range.get("begin_va")), 16) if function_range.get("begin_va") else va
        end = int(str(function_range.get("end_va")), 16) if function_range.get("end_va") else fallback_end
        blob = read_bytes(data, image_base, sections, begin, max(0, end - begin))
        return function_range, begin, end, blob

    final_combiner_range, final_combiner_begin, final_combiner_end, final_combiner_blob = range_blob(
        "title_accept_final_combiner_7ab170", 0x1407AB36E
    )
    final_wrapper_range, final_wrapper_begin, final_wrapper_end, final_wrapper_blob = range_blob(
        "title_accept_final_wrapper_9aa430", 0x1409AA4DF
    )
    branch_compose_range, branch_compose_begin, branch_compose_end, branch_compose_blob = range_blob(
        "slot_reset_branch_gate_chain_compose_78e0e0", 0x14078E1A7
    )
    branch_step_range, branch_step_begin, branch_step_end, branch_step_blob = range_blob(
        "slot_reset_branch_gate_chain_step_7927d0", 0x140792889
    )
    final_combiner_inner_range, final_combiner_inner_begin, final_combiner_inner_end, final_combiner_inner_blob = range_blob(
        "title_accept_final_combiner_inner_7abb40", 0x1407ABDC1
    )
    final_wrapper_inner_range, final_wrapper_inner_begin, final_wrapper_inner_end, final_wrapper_inner_blob = range_blob(
        "title_accept_final_wrapper_inner_78c530", 0x14078C5C6
    )
    branch_compose_inner_range, branch_compose_inner_begin, branch_compose_inner_end, branch_compose_inner_blob = range_blob(
        "title_accept_branch_compose_inner_7926e0", 0x1407927CE
    )
    branch_step_builder_range, branch_step_builder_begin, branch_step_builder_end, branch_step_builder_blob = range_blob(
        "title_accept_branch_step_builder_792970", 0x140792B66
    )
    branch_step_build_inner_range, branch_step_build_inner_begin, branch_step_build_inner_end, branch_step_build_inner_blob = range_blob(
        "title_accept_branch_step_build_inner_792100", 0x1407923E0
    )
    branch_step_status_vslot2_range, branch_step_status_vslot2_begin, branch_step_status_vslot2_end, branch_step_status_vslot2_blob = range_blob(
        "title_accept_branch_step_status_vslot2_7aa1f0", 0x1407AA371
    )
    branch_step_payload_vslot2_range, branch_step_payload_vslot2_begin, branch_step_payload_vslot2_end, branch_step_payload_vslot2_blob = range_blob(
        "title_accept_branch_step_payload_vslot2_792460", 0x14079253D
    )
    branch_step_payload_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["title_accept_branch_step_payload_vtable_aa2938"], 3
    )
    branch_step_status_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["title_accept_branch_step_status_vtable_aa2958"], 3
    )

    def calls_in(target_name: str) -> list[str]:
        return sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if update_begin <= int(str(ref.get("source_va")), 16) < update_end
            ],
            key=lambda value: int(value, 16),
        )

    def calls_in_range(target_name: str, begin: int, end: int) -> list[str]:
        return sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if begin <= int(str(ref.get("source_va")), 16) < end
            ],
            key=lambda value: int(value, 16),
        )

    task_enqueue_sources = calls_in("task_enqueue_7a7b60")
    task_enqueue_link_sources = calls_in("task_enqueue_link_7a7bb0")
    state_chain_sources = calls_in("selector_builder_chain_key_7a91e0")
    descriptor_wrapper_sources = calls_in("slot_reset_branch_gate_descriptor_wrapper_744a60")
    primary_chain_builder_sources = calls_in("title_accept_primary_chain_builder_7a72b0")
    aux_builder_ac620_sources = calls_in("title_accept_aux_builder_ac620")
    primary_builder_a6c70_sources = calls_in("title_accept_primary_builder_a6c70")
    owner_payload_builder_sources = calls_in("title_accept_owner_payload_builder_833880")
    final_owner_builder_sources = calls_in("title_accept_final_owner_builder_9aa2d0")
    final_combiner_sources = calls_in("title_accept_final_combiner_7ab170")
    final_wrapper_sources = calls_in("title_accept_final_wrapper_9aa430")
    final_cleanup_sources = calls_in("title_accept_final_cleanup_78cec0")
    branch_compose_sources = calls_in("slot_reset_branch_gate_chain_compose_78e0e0")
    branch_step_sources = calls_in("slot_reset_branch_gate_chain_step_7927d0")
    final_combiner_inner_sources = calls_in_range(
        "title_accept_final_combiner_inner_7abb40", final_combiner_begin, final_combiner_end
    )
    final_combiner_attach_sources = calls_in_range(
        "title_accept_attach_7418d0", final_combiner_begin, final_combiner_end
    )
    final_wrapper_inner_sources = calls_in_range(
        "title_accept_final_wrapper_inner_78c530", final_wrapper_begin, final_wrapper_end
    )
    branch_compose_inner_sources = calls_in_range(
        "title_accept_branch_compose_inner_7926e0", branch_compose_begin, branch_compose_end
    )
    branch_step_builder_sources = calls_in_range(
        "title_accept_branch_step_builder_792970", branch_step_begin, branch_step_end
    )
    branch_step_attach_sources = calls_in_range("title_accept_attach_7418d0", branch_step_begin, branch_step_end)
    final_combiner_append_sources = calls_in_range(
        "title_accept_final_combiner_append_7ac0b0", final_combiner_inner_begin, final_combiner_inner_end
    )
    branch_step_swap_sources = calls_in_range(
        "title_accept_branch_step_swap_7925d0", branch_step_builder_begin, branch_step_builder_end
    )
    branch_step_inner_builder_sources = calls_in_range(
        "title_accept_branch_step_build_inner_792100", branch_step_builder_begin, branch_step_builder_end
    )
    branch_step_payload_sources = calls_in_range(
        "title_accept_branch_step_payload_791b50", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    branch_step_condition_init_sources = calls_in_range(
        "title_accept_branch_step_condition_init_7923f0", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    branch_step_condition_add_sources = calls_in_range(
        "title_accept_branch_step_condition_add_793770", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    branch_step_condition_finish_sources = calls_in_range(
        "title_accept_branch_step_condition_finish_7936f0", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    branch_step_condition_cleanup_sources = calls_in_range(
        "title_accept_branch_step_condition_cleanup_791ed0", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    branch_step_inner_attach_sources = calls_in_range(
        "title_accept_branch_step_attach_7aa380", branch_step_build_inner_begin, branch_step_build_inner_end
    )
    status_vslot2_terminal_check_sources = calls_in_range(
        "descriptor_status_gt_one_7a9200", branch_step_status_vslot2_begin, branch_step_status_vslot2_end
    )
    status_vslot2_success_check_sources = calls_in_range(
        "descriptor_status_eq_two_7a9210", branch_step_status_vslot2_begin, branch_step_status_vslot2_end
    )
    payload_vslot2_terminal_check_sources = calls_in_range(
        "descriptor_status_gt_one_7a9200", branch_step_payload_vslot2_begin, branch_step_payload_vslot2_end
    )
    task_enqueue_source_set = set(task_enqueue_sources)
    task_enqueue_link_source_set = set(task_enqueue_link_sources)
    primary_trace_enqueue_sources_present = {"0x1409b26d3", "0x1409b2703"}.issubset(task_enqueue_source_set)
    primary_trace_link_source_present = "0x1409b26f6" in task_enqueue_link_source_set
    primary_trace_link_between_first_two_enqueues = False
    if primary_trace_enqueue_sources_present and primary_trace_link_source_present:
        primary_trace_link_between_first_two_enqueues = (
            task_enqueue_sources.index("0x1409b26d3")
            < task_enqueue_sources.index("0x1409b2703")
            and task_enqueue_link_sources.index("0x1409b26f6") == 0
        )
    return {
        "function_begin_va": update_range.get("begin_va"),
        "function_end_va": update_range.get("end_va"),
        "final_combiner_begin_va": final_combiner_range.get("begin_va"),
        "final_combiner_end_va": final_combiner_range.get("end_va"),
        "final_wrapper_begin_va": final_wrapper_range.get("begin_va"),
        "final_wrapper_end_va": final_wrapper_range.get("end_va"),
        "branch_compose_begin_va": branch_compose_range.get("begin_va"),
        "branch_compose_end_va": branch_compose_range.get("end_va"),
        "branch_step_begin_va": branch_step_range.get("begin_va"),
        "branch_step_end_va": branch_step_range.get("end_va"),
        "final_combiner_inner_begin_va": final_combiner_inner_range.get("begin_va"),
        "final_combiner_inner_end_va": final_combiner_inner_range.get("end_va"),
        "final_wrapper_inner_begin_va": final_wrapper_inner_range.get("begin_va"),
        "final_wrapper_inner_end_va": final_wrapper_inner_range.get("end_va"),
        "branch_compose_inner_begin_va": branch_compose_inner_range.get("begin_va"),
        "branch_compose_inner_end_va": branch_compose_inner_range.get("end_va"),
        "branch_step_builder_begin_va": branch_step_builder_range.get("begin_va"),
        "branch_step_builder_end_va": branch_step_builder_range.get("end_va"),
        "branch_step_build_inner_begin_va": branch_step_build_inner_range.get("begin_va"),
        "branch_step_build_inner_end_va": branch_step_build_inner_range.get("end_va"),
        "branch_step_status_vslot2_begin_va": branch_step_status_vslot2_range.get("begin_va"),
        "branch_step_status_vslot2_end_va": branch_step_status_vslot2_range.get("end_va"),
        "branch_step_payload_vslot2_begin_va": branch_step_payload_vslot2_range.get("begin_va"),
        "branch_step_payload_vslot2_end_va": branch_step_payload_vslot2_range.get("end_va"),
        "branch_step_payload_vtable_slots": branch_step_payload_vtable_slots,
        "branch_step_status_vtable_slots": branch_step_status_vtable_slots,
        "task_enqueue_sources": task_enqueue_sources,
        "task_enqueue_link_sources": task_enqueue_link_sources,
        "state_chain_sources": state_chain_sources,
        "descriptor_wrapper_sources": descriptor_wrapper_sources,
        "primary_chain_builder_sources": primary_chain_builder_sources,
        "aux_builder_ac620_sources": aux_builder_ac620_sources,
        "primary_builder_a6c70_sources": primary_builder_a6c70_sources,
        "owner_payload_builder_sources": owner_payload_builder_sources,
        "final_owner_builder_sources": final_owner_builder_sources,
        "final_combiner_sources": final_combiner_sources,
        "final_wrapper_sources": final_wrapper_sources,
        "final_cleanup_sources": final_cleanup_sources,
        "branch_compose_sources": branch_compose_sources,
        "branch_step_sources": branch_step_sources,
        "final_combiner_inner_sources": final_combiner_inner_sources,
        "final_combiner_attach_sources": final_combiner_attach_sources,
        "final_wrapper_inner_sources": final_wrapper_inner_sources,
        "branch_compose_inner_sources": branch_compose_inner_sources,
        "branch_step_builder_sources": branch_step_builder_sources,
        "branch_step_attach_sources": branch_step_attach_sources,
        "final_combiner_append_sources": final_combiner_append_sources,
        "branch_step_swap_sources": branch_step_swap_sources,
        "branch_step_inner_builder_sources": branch_step_inner_builder_sources,
        "branch_step_payload_sources": branch_step_payload_sources,
        "branch_step_condition_init_sources": branch_step_condition_init_sources,
        "branch_step_condition_add_sources": branch_step_condition_add_sources,
        "branch_step_condition_finish_sources": branch_step_condition_finish_sources,
        "branch_step_condition_cleanup_sources": branch_step_condition_cleanup_sources,
        "branch_step_inner_attach_sources": branch_step_inner_attach_sources,
        "status_vslot2_terminal_check_sources": status_vslot2_terminal_check_sources,
        "status_vslot2_success_check_sources": status_vslot2_success_check_sources,
        "payload_vslot2_terminal_check_sources": payload_vslot2_terminal_check_sources,
        "primary_trace_enqueue_sources_present": primary_trace_enqueue_sources_present,
        "primary_trace_link_source_present": primary_trace_link_source_present,
        "primary_trace_link_between_first_two_enqueues": primary_trace_link_between_first_two_enqueues,
        "late_chain_link_sources_present": {"0x1409b2842", "0x1409b2853", "0x1409b2864"}.issubset(
            task_enqueue_link_source_set
        ),
        "primary_chain_builder_source_present": primary_chain_builder_sources == ["0x1409b26e6"],
        "primary_link_uses_first_enqueue_result": b"\x48\x8b\xd8\x48\x8d\x95\x40\x01\x00\x00" in update_blob
        and b"\x4c\x8b\xc3\x48\x8d\x55\x98\x48\x8b\xc8" in update_blob,
        "primary_link_feeds_second_enqueue": b"\xe8\xb5\x54\xdf\xff\x90\x48\x8d\x55\x40\x48\x8b\xc8\xe8\x58\x54\xdf\xff" in update_blob,
        "second_primary_enqueue_saved_r15": b"\x48\x8d\x55\x40\x48\x8b\xc8\xe8\x58\x54\xdf\xff\x4c\x8b\xf8" in update_blob,
        "state_chain_args_use_zero_and_add2": b"\x45\x33\xc0\x41\x8d\x54\x24\x02\x48\x8d\x4d\xd0" in update_blob,
        "state_descriptor_wrapper_uses_chain_result": b"\x4c\x8b\x45\xd0\x48\x8d\x95\x80\x00\x00\x00\x48\x8d\x4d\x90" in update_blob,
        "state_descriptor_wrapper_result_enqueued_saved_r14": b"\xe8\xe1\x22\xd9\xff\x90\x48\x8d\x55\xd8\x48\x8b\xc8\xe8\xd4\x53\xdf\xff\x4c\x8b\xf0" in update_blob,
        "late_aux_builder_sources_present": aux_builder_ac620_sources == ["0x1409b27a0"]
        and primary_builder_a6c70_sources == ["0x1409b27c9"]
        and owner_payload_builder_sources == ["0x1409b2801"],
        "late_aux_enqueues_saved_registers": all(
            needle in update_blob
            for needle in [
                b"\xe8\x7b\x9e\xff\xff\x90\x48\x8d\x55\xe0\x48\x8b\xc8\xe8\xae\x53\xdf\xff\x48\x8b\xf0",
                b"\xe8\xa2\x44\xff\xff\x90\x48\x8d\x55\xe8\x48\x8b\xc8\xe8\x85\x53\xdf\xff\x48\x8b\xf8",
                b"\xe8\x7a\x10\xe8\xff\x90\x48\x8d\x55\xf0\x48\x8b\xc8\xe8\x4d\x53\xdf\xff\x48\x8b\xd8",
            ]
        ),
        "late_link_chain_folds_aux_results_to_final_enqueue": b"\x4c\x8b\xc3\x48\x8d\x54\x24\x68\x48\x8b\xc8\xe8\x69\x53\xdf\xff\x90\x4c\x8b\xc7\x48\x8d\x54\x24\x60\x48\x8b\xc8\xe8\x58\x53\xdf\xff\x90\x4c\x8b\xc6\x48\x8d\x54\x24\x58\x48\x8b\xc8\xe8\x47\x53\xdf\xff\x90\x48\x8d\x55\x00\x48\x8b\xc8\xe8\xea\x52\xdf\xff\x48\x8b\xd8" in update_blob,
        "final_owner_builder_source_present": final_owner_builder_sources == ["0x1409b2884"],
        "final_combiner_source_present": final_combiner_sources == ["0x1409b28a4"],
        "final_combiner_uses_late_link_result_and_owner_result": b"\x48\x8d\x4c\x24\x40\x48\x89\x4c\x24\x20\x4c\x8d\x4c\x24\x48\x4c\x8b\xc3\x48\x8b\xd0\x48\x8d\x4c\x24\x50" in update_blob,
        "final_combiner_result_enqueued": b"\xe8\xc7\x88\xdf\xff\x90\x48\x8d\x55\x10\x48\x8b\xc8\xe8\xaa\x52\xdf\xff" in update_blob,
        "final_enqueue_result_passed_to_wrapper": final_wrapper_sources == ["0x1409b28c1"]
        and b"\x41\xb0\x01\x48\x8b\xd0\x48\x8d\x4d\x70" in update_blob,
        "final_wrapper_chains_r14_r15": branch_compose_sources == ["0x1409b28d4"]
        and branch_step_sources == ["0x1409b28e5"]
        and b"\x4d\x8b\xc6\x48\x8d\x95\x28\x01\x00\x00\x48\x8b\xc8" in update_blob
        and b"\x4d\x8b\xc7\x48\x8d\x54\x24\x38\x48\x8b\xc8" in update_blob,
        "final_cleanup_after_chain_source_present": final_cleanup_sources == ["0x1409b28f2"],
        "final_combiner_body_range_mapped": final_combiner_range.get("begin_va") == "0x1407ab170"
        and final_combiner_range.get("end_va") == "0x1407ab36f",
        "final_combiner_calls_inner_and_attach": final_combiner_inner_sources == ["0x1407ab255"]
        and final_combiner_attach_sources == ["0x1407ab261"],
        "final_combiner_captures_four_sources": b"\x4d\x8b\xf1\x49\x8b\xf8\x48\x8b\xf2\x4c\x8b\xf9" in final_combiner_blob,
        "final_combiner_inner_receives_four_locals": b"\x48\x8d\x45\xb7\x48\x89\x44\x24\x20\x4c\x8d\x4d\xbf\x4c\x8d\x45\xc7\x48\x8d\x55\xcf\x48\x8d\x4d\xd7" in final_combiner_blob,
        "final_combiner_clears_transferred_slots": all(
            needle in final_combiner_blob
            for needle in [b"\x4c\x89\x2e", b"\x4c\x89\x2f", b"\x4d\x89\x2e", b"\x4d\x89\x2c\x24"]
        ),
        "final_wrapper_body_range_mapped": final_wrapper_range.get("begin_va") == "0x1409aa430"
        and final_wrapper_range.get("end_va") == "0x1409aa4e0",
        "final_wrapper_calls_inner_with_flag": final_wrapper_inner_sources == ["0x1409aa489"]
        and b"\x44\x0f\xb6\xc7\x48\x8d\x54\x24\x78\x48\x8b\xcb" in final_wrapper_blob,
        "final_wrapper_clears_source_slot": b"\x48\xc7\x06\x00\x00\x00\x00" in final_wrapper_blob,
        "branch_compose_body_calls_inner": branch_compose_inner_sources == ["0x14078e15b"]
        and b"\x44\x0f\xb6\x43\x08" in branch_compose_blob,
        "branch_step_body_builds_and_attaches": branch_step_builder_sources == ["0x140792831"]
        and branch_step_attach_sources == ["0x14079283d"],
        "final_combiner_inner_short_circuits_all_empty": b"\x4c\x39\x2a\x75\x21\x4d\x39\x28\x75\x1c\x4d\x39\x29\x75\x17\x4d\x39\x2f\x75\x12\x4c\x89\x29" in final_combiner_inner_blob,
        "final_combiner_inner_allocates_composite_node": function_has_rip_lea_any_reg_to(
            data,
            image_base,
            sections,
            final_combiner_inner_begin,
            0x142AA9428,
            max(0, final_combiner_inner_end - final_combiner_inner_begin),
        )
        and b"\x41\x8d\x50\x01\xe8\xe8\xd5\xff\xff" in final_combiner_inner_blob,
        "final_combiner_inner_appends_four_sources": final_combiner_append_sources
        == ["0x1407abc31", "0x1407abc5b", "0x1407abc85", "0x1407abcaf"],
        "final_combiner_inner_stores_output_and_releases_inputs": b"\x49\x89\x1c\x24" in final_combiner_inner_blob
        and all(
            needle in final_combiner_inner_blob
            for needle in [b"\x4c\x89\x2f", b"\x4c\x89\x2e", b"\x4d\x89\x2e", b"\x4d\x89\x2f"]
        ),
        "final_wrapper_inner_transfers_source_and_flag": b"\x48\x8b\x0a\x48\x89\x0b" in final_wrapper_inner_blob
        and b"\x40\x88\x7b\x08" in final_wrapper_inner_blob
        and b"\x48\xc7\x06\x00\x00\x00\x00" in final_wrapper_inner_blob,
        "branch_compose_inner_transfers_two_sources_and_flag": b"\x48\x8b\x0a\x48\x89\x0b" in branch_compose_inner_blob
        and b"\x48\x8b\x0e\x48\x89\x4b\x08" in branch_compose_inner_blob
        and b"\x40\x88\x7b\x10" in branch_compose_inner_blob
        and b"\x49\xc7\x06\x00\x00\x00\x00" in branch_compose_inner_blob
        and b"\x48\xc7\x06\x00\x00\x00\x00" in branch_compose_inner_blob,
        "branch_step_builder_reorders_inputs_on_false_flag": branch_step_swap_sources == ["0x1407929f7"]
        and b"\x41\x80\x7e\x10\x00\x75\x15" in branch_step_builder_blob,
        "branch_step_builder_calls_inner_with_three_locals": branch_step_inner_builder_sources == ["0x140792a64"]
        and b"\x4c\x8d\x4d\xb0\x4c\x8d\x45\xb8\x48\x8d\x55\xc0\x49\x8b\xcc" in branch_step_builder_blob,
        "branch_step_build_inner_allocates_status_node": function_has_rip_lea_any_reg_to(
            data,
            image_base,
            sections,
            branch_step_build_inner_begin,
            0x142AA2958,
            max(0, branch_step_build_inner_end - branch_step_build_inner_begin),
        )
        and b"\xc7\x43\x68\x01\x00\x00\x00" in branch_step_build_inner_blob,
        "branch_step_build_inner_builds_payload_and_attaches": branch_step_payload_sources == ["0x1407921f5"]
        and branch_step_inner_attach_sources == ["0x14079221b", "0x1407922b1"],
        "branch_step_build_inner_builds_condition_chain": branch_step_condition_init_sources == ["0x140792275"]
        and branch_step_condition_add_sources == ["0x140792288", "0x140792298"]
        and branch_step_condition_finish_sources == ["0x1407922a5"]
        and b"\x4c\x8d\x44\x24\x20\xba\x01\x00\x00\x00\x48\x8b\xc8" in branch_step_build_inner_blob
        and b"\x4c\x8d\x44\x24\x30\x33\xd2\x48\x8b\xc8" in branch_step_build_inner_blob,
        "branch_step_build_inner_cleans_condition_temp": branch_step_condition_cleanup_sources == ["0x1407922bc"]
        and b"\x4c\x8b\x64\x24\x60\x4d\x85\xe4" in branch_step_build_inner_blob,
        "branch_step_vtables_link_payload_to_status_sequence": [row["value_va"] for row in branch_step_payload_vtable_slots]
        == ["0x140744d90", "0x140792050", "0x140792460"]
        and [row["value_va"] for row in branch_step_status_vtable_slots]
        == ["0x140744d90", "0x140792090", "0x1407aa1f0"],
        "branch_step_payload_vslot2_sets_terminal_status": payload_vslot2_terminal_check_sources == ["0x1407924d5"]
        and b"\xff\x50\x10\x48\x8d\x4c\x24\x70\xe8" in branch_step_payload_vslot2_blob
        and b"\x41\x8d\x50\x02\x48\x8b\xcf\xe8" in branch_step_payload_vslot2_blob
        and b"\x41\x8d\x50\x01\x48\x8b\xcf\xe8" in branch_step_payload_vslot2_blob,
        "branch_step_payload_vslot2_writes_condition_result": b"\x49\x8b\x46\x18\x89\x08" in branch_step_payload_vslot2_blob
        and b"\xe8\x28\x6d\x01\x00\x0f\xb6\xc8" in branch_step_payload_vslot2_blob,
        "branch_step_status_vslot2_iterates_children_until_done": status_vslot2_terminal_check_sources == ["0x1407aa2c4"]
        and status_vslot2_success_check_sources == ["0x1407aa2d0"]
        and b"\xff\x50\x10\x48\x8b\x08\x48\x89\x0e" in branch_step_status_vslot2_blob
        and b"\xff\x47\x10" in branch_step_status_vslot2_blob,
        "branch_step_status_vslot2_stops_on_nonterminal_or_failed_child": b"\x84\xc0\x74\x55" in branch_step_status_vslot2_blob
        and b"\x84\xc0\x74\x49" in branch_step_status_vslot2_blob,
        "builder_state_descriptor_sources_present": state_chain_sources == ["0x1409b2765"]
        and descriptor_wrapper_sources == ["0x1409b277a"],
        "builds_title_accept_descriptor_vtables": function_has_rip_lea_any_reg_to(
            data, image_base, sections, update_begin, 0x1429E2848, max(0, update_end - update_begin)
        )
        and function_has_rip_lea_any_reg_to(
            data, image_base, sections, update_begin, 0x142B26708, max(0, update_end - update_begin)
        ),
        "uses_owner_payload_fields": all(
            needle in update_blob
            for needle in [
                b"\x49\x8b\x8d\x38\x0a\x00\x00",
                b"\x49\x8d\x4d\x50",
                b"\x49\x8d\x95\x48\x0a\x00\x00",
            ]
        ),
        "task_enqueue_fanout_count": len(task_enqueue_sources),
        "task_enqueue_fanout_has_expected_late_sources": {
            "0x1409b2787",
            "0x1409b27ad",
            "0x1409b27d6",
            "0x1409b280e",
            "0x1409b2871",
            "0x1409b28b1",
        }.issubset(task_enqueue_source_set),
    }


def read_slot_reset_end_flow_branch_gate_table_init_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
) -> dict[str, Any]:
    init_va = TARGETS["slot_reset_end_flow_branch_gate_table_init_0a2e10"]
    table_va = TARGETS["slot_reset_end_flow_branch_gate_table_global_43d6f9d0"]
    init_range = find_pdata_range_for_pc(data, image_base, sections, init_va)
    init_begin = int(str(init_range.get("begin_va")), 16) if init_range.get("begin_va") else init_va
    init_end = int(str(init_range.get("end_va")), 16) if init_range.get("end_va") else init_va + 0xA7
    blob = read_bytes(data, image_base, sections, init_begin, max(0, init_end - init_begin))
    refs = scan_rip_relative_refs_to_va(data, image_base, sections, table_va)
    stores: list[dict[str, Any]] = []
    for index in range(max(0, len(blob) - 7)):
        if blob[index : index + 3] != b"\x48\x8d\x05":
            continue
        value_va = init_begin + index + 7 + struct.unpack_from("<i", blob, index + 3)[0]
        store_index = index + 7
        if blob[store_index : store_index + 3] != b"\x48\x89\x05":
            continue
        store_va = init_begin + store_index + 7 + struct.unpack_from("<i", blob, store_index + 3)[0]
        if not (table_va <= store_va < table_va + 0x50):
            continue
        offset = store_va - table_va
        stores.append(
            {
                "source_va": f"0x{init_begin + index:x}",
                "store_va": f"0x{store_va:x}",
                "store_offset": f"0x{offset:x}",
                "entry_index": offset // 16,
                "entry_slot": "handler" if offset % 16 == 0 else "label",
                "value_va": f"0x{value_va:x}",
                "label_text": read_utf16z(data, image_base, sections, value_va) if offset % 16 == 8 else None,
            }
        )
    entries: list[dict[str, Any]] = []
    for entry_index in sorted({int(row["entry_index"]) for row in stores}):
        handler = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "handler"), None)
        label = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "label"), None)
        entries.append(
            {
                "entry_index": entry_index,
                "handler_va": handler.get("value_va") if handler else None,
                "handler_source_va": handler.get("source_va") if handler else None,
                "label_va": label.get("value_va") if label else None,
                "label_source_va": label.get("source_va") if label else None,
                "label_text": label.get("label_text") if label else None,
            }
        )
    return {
        "function_begin_va": init_range.get("begin_va"),
        "function_end_va": init_range.get("end_va"),
        "table_base_va": f"0x{table_va:x}",
        "zeroes_table_base": function_has_rip_lea_any_reg_to(data, image_base, sections, init_begin, table_va, max(0, init_end - init_begin)),
        "zero_size_bytes": 0x50
        if (b"\x41\xb8\x50\x00\x00\x00" in blob or (b"\x33\xd2" in blob and b"\x44\x8d\x42\x50" in blob))
        else None,
        "refs": refs,
        "stores": stores,
        "entries": entries,
        "bytes_hex": blob.hex(),
    }


def read_finish_gate_synchronization_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
) -> dict[str, Any]:
    finish_gate_va = TARGETS["slot_reset_finish_gate_global_43d856a0"]
    refs = sorted(
        scan_rip_relative_refs_to_va(data, image_base, sections, finish_gate_va),
        key=lambda ref: int(str(ref.get("source_va")), 16),
    )
    refs_by_function: dict[str, list[str]] = {}
    for ref in refs:
        function_begin = str(ref.get("function_begin_va"))
        refs_by_function.setdefault(function_begin, []).append(str(ref.get("source_va")))
    source_set = {str(ref.get("source_va")) for ref in refs}
    return {
        "finish_gate_va": f"0x{finish_gate_va:x}",
        "ref_count": len(refs),
        "ref_sources": [str(ref.get("source_va")) for ref in refs],
        "refs_by_function": refs_by_function,
        "save_request_gate_sources": sorted(
            source_set.intersection({"0x14067a3d4", "0x14067a3e4", "0x14067a509", "0x14067a66a"}),
            key=lambda value: int(value, 16),
        ),
        "move_map_dispatch_source": "0x140afb8e1" if "0x140afb8e1" in source_set else None,
        "ending_wait_source": "0x140ae5d2a" if "0x140ae5d2a" in source_set else None,
        "ending_menu_job_wait_source": "0x140ae546f" if "0x140ae546f" in source_set else None,
        "title_end_flow_wait_source": "0x140b0cd41" if "0x140b0cd41" in source_set else None,
        "title_menu_job_wait_source": "0x140b0d526" if "0x140b0d526" in source_set else None,
        "ending_stage_functions": {
            "0x140ae5d10": refs_by_function.get("0x140ae5d10", []),
            "0x140ae5390": refs_by_function.get("0x140ae5390", []),
        },
        "title_stage_functions": {
            "0x140b0ccc0": refs_by_function.get("0x140b0ccc0", []),
            "0x140b0d400": refs_by_function.get("0x140b0d400", []),
        },
    }


def read_slot_reset_end_flow_branch_gate_state_handlers_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    wait_va = TARGETS["slot_reset_end_flow_branch_gate_wait_handler_ae5d10"]
    menu_va = TARGETS["slot_reset_end_flow_branch_gate_menu_job_wait_ae5390"]
    finish_va = TARGETS["slot_reset_end_flow_branch_gate_finish_ae5380"]
    wait_range = find_pdata_range_for_pc(data, image_base, sections, wait_va)
    menu_range = find_pdata_range_for_pc(data, image_base, sections, menu_va)
    finish_range = find_pdata_range_for_pc(data, image_base, sections, finish_va)
    wait_begin = int(str(wait_range.get("begin_va")), 16) if wait_range.get("begin_va") else wait_va
    wait_end = int(str(wait_range.get("end_va")), 16) if wait_range.get("end_va") else wait_va + 0x3B
    menu_begin = int(str(menu_range.get("begin_va")), 16) if menu_range.get("begin_va") else menu_va
    menu_end = int(str(menu_range.get("end_va")), 16) if menu_range.get("end_va") else menu_va + 0x102
    # The tiny Finish handler is not represented by a reliable pdata entry on this build;
    # find_pdata_range_for_pc lands in a huge unrelated range. Use the exact table target.
    finish_begin = finish_va
    finish_end = finish_va + 8
    wait_blob = read_bytes(data, image_base, sections, wait_begin, max(0, wait_end - wait_begin))
    menu_blob = read_bytes(data, image_base, sections, menu_begin, max(0, menu_end - menu_begin))
    finish_blob = read_bytes(data, image_base, sections, finish_begin, max(0, finish_end - finish_begin))

    def calls_in(begin: int, end: int, targets: list[str]) -> dict[str, list[str]]:
        return {
            target_name: sorted(
                [
                    str(ref.get("source_va"))
                    for ref in rel32_refs.get(target_name, [])
                    if begin <= int(str(ref.get("source_va")), 16) < end
                ],
                key=lambda value: int(value, 16),
            )
            for target_name in targets
        }

    wait_calls = calls_in(
        wait_begin,
        wait_end,
        [
            "slot_reset_end_flow_branch_gate_wait_probe_80d5c0",
            "slot_reset_end_flow_branch_gate_countdown_done_ae5d50",
            "slot_reset_branch_gate_submit_stage_ae5e60",
        ],
    )
    menu_calls = calls_in(
        menu_begin,
        menu_end,
        [
            "slot_reset_menu_job_wait_task_submit_733f20",
            "slot_reset_menu_job_wait_global_toggle_7663c0",
            "slot_reset_menu_job_wait_queue_7a9600",
            "slot_reset_branch_gate_submit_stage_ae5e60",
        ],
    )
    finish_gate_refs = scan_rip_relative_refs_to_va(
        data, image_base, sections, TARGETS["slot_reset_finish_gate_global_43d856a0"]
    )
    return {
        "wait_function_begin_va": wait_range.get("begin_va"),
        "wait_function_end_va": wait_range.get("end_va"),
        "menu_job_wait_function_begin_va": menu_range.get("begin_va"),
        "menu_job_wait_function_end_va": menu_range.get("end_va"),
        "finish_function_begin_va": f"0x{finish_begin:x}",
        "finish_function_end_va": f"0x{finish_end:x}",
        "finish_pdata_begin_va": finish_range.get("begin_va"),
        "finish_pdata_end_va": finish_range.get("end_va"),
        "wait_calls_by_target": wait_calls,
        "menu_job_wait_calls_by_target": menu_calls,
        "wait_probe_false_advances_countdown_done": bool(
            wait_calls.get("slot_reset_end_flow_branch_gate_wait_probe_80d5c0") == ["0x140ae5d19"]
            and wait_calls.get("slot_reset_end_flow_branch_gate_countdown_done_ae5d50") == ["0x140ae5d25"]
            and b"\x84\xc0" in wait_blob
            and b"\x75\x08" in wait_blob
        ),
        "wait_finish_gate_sets_stage4": bool(
            any(ref.get("source_va") == "0x140ae5d2a" for ref in finish_gate_refs)
            and b"\xba\x04\x00\x00\x00" in wait_blob
            and wait_calls.get("slot_reset_branch_gate_submit_stage_ae5e60") == ["0x140ae5d40"]
        ),
        "menu_job_wait_builds_timed_task_from_arg_plus8": bool(
            b"\x48\x8b\xfa" in menu_blob
            and b"\xf3\x0f\x10\x42\x08" in menu_blob
            and menu_calls.get("slot_reset_menu_job_wait_task_submit_733f20") == ["0x140ae53f1"]
        ),
        "menu_job_wait_submits_owner_b8_timed_task": bool(
            b"\x48\x81\xc1\xb8\x00\x00\x00" in menu_blob
            and menu_calls.get("slot_reset_menu_job_wait_task_submit_733f20") == ["0x140ae53f1"]
        ),
        "menu_job_wait_toggles_global_and_queues_owner108": bool(
            b"\xb2\x01" in menu_blob
            and menu_calls.get("slot_reset_menu_job_wait_global_toggle_7663c0") == ["0x140ae5432"]
            and b"\x48\x8d\x8b\x08\x01\x00\x00" in menu_blob
            and menu_calls.get("slot_reset_menu_job_wait_queue_7a9600") == ["0x140ae546a"]
        ),
        "menu_job_wait_finish_gate_sets_stage4": bool(
            any(ref.get("source_va") == "0x140ae546f" for ref in finish_gate_refs)
            and b"\xba\x04\x00\x00\x00" in menu_blob
            and menu_calls.get("slot_reset_branch_gate_submit_stage_ae5e60") == ["0x140ae5480"]
        ),
        "finish_sets_owner_4c_minus_one": b"\xc7\x41\x4c\xff\xff\xff\xff" in finish_blob,
        "wait_bytes_hex": wait_blob.hex(),
        "menu_job_wait_bytes_hex": menu_blob.hex(),
        "finish_bytes_hex": finish_blob.hex(),
    }


def read_slot_reset_end_flow_branch_gate_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    gate_va = 0x143D6F9C0
    refs = scan_rip_relative_refs_to_va(data, image_base, sections, gate_va)
    clear_countdown_va = TARGETS["slot_reset_end_flow_branch_gate_countdown_clear_ae5cd0"]
    clear_countdown_range = find_pdata_range_for_pc(data, image_base, sections, clear_countdown_va)
    clear_begin = int(str(clear_countdown_range.get("begin_va")), 16) if clear_countdown_range.get("begin_va") else clear_countdown_va
    clear_end = int(str(clear_countdown_range.get("end_va")), 16) if clear_countdown_range.get("end_va") else clear_countdown_va + 0x3F
    clear_blob = read_bytes(data, image_base, sections, clear_begin, max(0, clear_end - clear_begin))
    done_range = find_pdata_range_for_pc(data, image_base, sections, TARGETS["slot_reset_end_flow_branch_gate_countdown_done_ae5d50"])
    done_begin = int(str(done_range.get("begin_va")), 16) if done_range.get("begin_va") else TARGETS["slot_reset_end_flow_branch_gate_countdown_done_ae5d50"]
    done_end = int(str(done_range.get("end_va")), 16) if done_range.get("end_va") else TARGETS["slot_reset_end_flow_branch_gate_countdown_done_ae5d50"] + 0x109
    done_blob = read_bytes(data, image_base, sections, done_begin, max(0, done_end - done_begin))
    tiny_clear_blob = read_bytes(data, image_base, sections, TARGETS["slot_reset_end_flow_branch_gate_tiny_clear_ae6350"], 8)
    tiny_set_blob = read_bytes(data, image_base, sections, TARGETS["slot_reset_end_flow_branch_gate_tiny_set_ae6360"], 8)
    tiny_wrapper_table_entries = read_qwords(data, image_base, sections, 0x142B5AF40, 12)
    builder_va = 0x140AE54A0
    builder_range = find_pdata_range_for_pc(data, image_base, sections, builder_va)
    builder_begin = int(str(builder_range.get("begin_va")), 16) if builder_range.get("begin_va") else builder_va
    builder_end = int(str(builder_range.get("end_va")), 16) if builder_range.get("end_va") else builder_va + 0x824
    builder_blob = read_bytes(data, image_base, sections, builder_begin, max(0, builder_end - builder_begin))
    builder_table_refs = {
        "set_table_plus8_142b5af48": [
            ref
            for ref in scan_rip_relative_refs_to_va(data, image_base, sections, 0x142B5AF48)
            if ref.get("function_begin_va") == "0x140ae54a0"
        ],
        "clear_table_142b5af80": [
            ref
            for ref in scan_rip_relative_refs_to_va(data, image_base, sections, 0x142B5AF80)
            if ref.get("function_begin_va") == "0x140ae54a0"
        ],
        "sibling_table_plus8_142b5afb8": [
            ref
            for ref in scan_rip_relative_refs_to_va(data, image_base, sections, 0x142B5AFB8)
            if ref.get("function_begin_va") == "0x140ae54a0"
        ],
    }
    builder_calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if builder_begin <= int(str(ref.get("source_va")), 16) < builder_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in [
            "selector_builder_chain_key_7a91e0",
            "slot_reset_branch_gate_descriptor_wrapper_744a60",
            "task_enqueue_7a7b60",
            "task_enqueue_link_7a7bb0",
            "slot_reset_branch_gate_resource_75fbd0",
            "slot_reset_branch_gate_timed_job_7b73d0",
            "slot_reset_branch_gate_chain_start_ae5180",
            "slot_reset_branch_gate_chain_compose_78e0e0",
            "slot_reset_branch_gate_chain_step_7927d0",
            "slot_reset_branch_gate_resource_762d50",
            "slot_reset_branch_gate_job_7bae20",
            "slot_reset_branch_gate_condition_ae5230",
            "slot_reset_branch_gate_chain_step_7928a0",
            "slot_reset_branch_gate_submit_ae6520",
        ]
    }
    submit_range = find_pdata_range_for_pc(data, image_base, sections, TARGETS["slot_reset_branch_gate_submit_ae6520"])
    submit_begin = int(str(submit_range.get("begin_va")), 16) if submit_range.get("begin_va") else TARGETS["slot_reset_branch_gate_submit_ae6520"]
    submit_end = int(str(submit_range.get("end_va")), 16) if submit_range.get("end_va") else TARGETS["slot_reset_branch_gate_submit_ae6520"] + 0xDF
    submit_blob = read_bytes(data, image_base, sections, submit_begin, max(0, submit_end - submit_begin))
    stage_range = find_pdata_range_for_pc(data, image_base, sections, TARGETS["slot_reset_branch_gate_submit_stage_ae5e60"])
    stage_begin = int(str(stage_range.get("begin_va")), 16) if stage_range.get("begin_va") else TARGETS["slot_reset_branch_gate_submit_stage_ae5e60"]
    stage_end = int(str(stage_range.get("end_va")), 16) if stage_range.get("end_va") else TARGETS["slot_reset_branch_gate_submit_stage_ae5e60"] + 0x109
    stage_blob = read_bytes(data, image_base, sections, stage_begin, max(0, stage_end - stage_begin))
    submit_calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if submit_begin <= int(str(ref.get("source_va")), 16) < submit_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in ["slot_reset_branch_gate_submit_attach_7a9460", "slot_reset_branch_gate_submit_stage_ae5e60"]
    }
    call_targets = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if clear_begin <= int(str(ref.get("source_va")), 16) < clear_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in ["task_event_80dc10", "slot_reset_end_flow_branch_gate_countdown_done_ae5d50"]
    }
    return {
        "gate_va": f"0x{gate_va:x}",
        "refs": refs,
        "ref_sources": sorted([str(ref.get("source_va")) for ref in refs], key=lambda value: int(value, 16)),
        "clear_countdown_function_begin_va": clear_countdown_range.get("begin_va"),
        "clear_countdown_function_end_va": clear_countdown_range.get("end_va"),
        "clear_countdown_calls_by_target": call_targets,
        "clear_countdown_clears_gate_first": b"\xc6\x05" in clear_blob
        and any(ref.get("source_va") == "0x140ae5cd6" and ref.get("bytes_hex", "").endswith("00") for ref in refs),
        "clear_countdown_uses_owner_110_countdown": (b"\x8b\x81\x10\x01\x00\x00" in clear_blob or b"\x8b\x83\x10\x01\x00\x00" in clear_blob)
        and b"\xff\xc8" in clear_blob
        and (b"\x89\x81\x10\x01\x00\x00" in clear_blob or b"\x89\x83\x10\x01\x00\x00" in clear_blob),
        "clear_countdown_jumps_done_when_zero": bool(
            b"\xe9" in clear_blob
            and call_targets.get("slot_reset_end_flow_branch_gate_countdown_done_ae5d50") == ["0x140ae5d0a"]
        ),
        "done_function_begin_va": done_range.get("begin_va"),
        "done_function_end_va": done_range.get("end_va"),
        "done_increments_owner_4c": b"\xff\x41\x4c" in done_blob,
        "done_bounds_owner_48_plus_one_le_6": b"\x8b\x41\x48" in done_blob
        and b"\xff\xc0" in done_blob
        and b"\x83\xf8\x06" in done_blob,
        "tiny_clear_wrapper_clears_gate": tiny_clear_blob.startswith(b"\xc6\x05")
        and tiny_clear_blob[6:7] == b"\x00"
        and tiny_clear_blob.endswith(b"\xc3"),
        "tiny_set_wrapper_sets_gate": tiny_set_blob.startswith(b"\xc6\x05")
        and tiny_set_blob[6:7] == b"\x01"
        and tiny_set_blob.endswith(b"\xc3"),
        "endflow_read_ref_source": "0x140b0cd21" if any(ref.get("source_va") == "0x140b0cd21" for ref in refs) else None,
        "tiny_wrapper_table_entries": tiny_wrapper_table_entries,
        "builder_function_begin_va": builder_range.get("begin_va"),
        "builder_function_end_va": builder_range.get("end_va"),
        "builder_table_refs": builder_table_refs,
        "builder_calls_by_target": builder_calls_by_target,
        "builder_has_resource_ids_6ddd1_6ddd0_1061": b"\xba\xd1\xdd\x06\x00" in builder_blob
        and b"\xba\xd0\xdd\x06\x00" in builder_blob
        and b"\xba\x61\x10\x00\x00" in builder_blob,
        "builder_has_timed_param_blocks_64_2_1": b"\xc7\x45\xe0\x64\x00\x00\x00" in builder_blob
        and b"\xc7\x45\xe4\x02\x00\x00\x00" in builder_blob
        and b"\xc7\x45\xe8\x01\x00\x00\x00" in builder_blob
        and b"\xc7\x45\xf8\x64\x00\x00\x00" in builder_blob,
        "builder_chain_helper_sources": {
            "resource_75fbd0": builder_calls_by_target.get("slot_reset_branch_gate_resource_75fbd0", []),
            "timed_job_7b73d0": builder_calls_by_target.get("slot_reset_branch_gate_timed_job_7b73d0", []),
            "chain_start_ae5180": builder_calls_by_target.get("slot_reset_branch_gate_chain_start_ae5180", []),
            "chain_compose_78e0e0": builder_calls_by_target.get("slot_reset_branch_gate_chain_compose_78e0e0", []),
            "chain_step_7927d0": builder_calls_by_target.get("slot_reset_branch_gate_chain_step_7927d0", []),
            "resource_762d50": builder_calls_by_target.get("slot_reset_branch_gate_resource_762d50", []),
            "job_7bae20": builder_calls_by_target.get("slot_reset_branch_gate_job_7bae20", []),
            "condition_ae5230": builder_calls_by_target.get("slot_reset_branch_gate_condition_ae5230", []),
            "chain_step_7928a0": builder_calls_by_target.get("slot_reset_branch_gate_chain_step_7928a0", []),
            "submit_ae6520": builder_calls_by_target.get("slot_reset_branch_gate_submit_ae6520", []),
        },
        "submit_function_begin_va": submit_range.get("begin_va"),
        "submit_function_end_va": submit_range.get("end_va"),
        "submit_calls_by_target": submit_calls_by_target,
        "submit_captures_owner_and_chain_ptr": b"\x48\x8b\xfa" in submit_blob and b"\x48\x8b\xf1" in submit_blob,
        "submit_attaches_chain_to_owner_108": bool(
            b"\x48\x8d\x8e\x08\x01\x00\x00" in submit_blob
            and submit_calls_by_target.get("slot_reset_branch_gate_submit_attach_7a9460") == ["0x140ae656d"]
        ),
        "submit_advances_owner_stage_3": bool(
            b"\xba\x03\x00\x00\x00" in submit_blob
            and submit_calls_by_target.get("slot_reset_branch_gate_submit_stage_ae5e60") == ["0x140ae65b3"]
        ),
        "submit_clears_source_chain_ptr": b"\x48\xc7\x07\x00\x00\x00\x00" in submit_blob,
        "stage_function_begin_va": stage_range.get("begin_va"),
        "stage_function_end_va": stage_range.get("end_va"),
        "stage_writes_owner_4c_from_edx": b"\x89\x51\x4c" in stage_blob,
        "stage_bounds_owner_48_plus_one_le_6": b"\x8b\x41\x48" in stage_blob
        and b"\xff\xc0" in stage_blob
        and b"\x83\xf8\x06" in stage_blob,
        "tiny_set_table_entry_va": "0x142b5af58"
        if any(entry.get("entry_va") == "0x142b5af58" and entry.get("value_va") == "0x140ae6360" for entry in tiny_wrapper_table_entries)
        else None,
        "tiny_clear_table_entry_va": "0x142b5af90"
        if any(entry.get("entry_va") == "0x142b5af90" and entry.get("value_va") == "0x140ae6350" for entry in tiny_wrapper_table_entries)
        else None,
        "clear_countdown_bytes_hex": clear_blob.hex(),
        "tiny_clear_bytes_hex": tiny_clear_blob.hex(),
        "tiny_set_bytes_hex": tiny_set_blob.hex(),
    }


def read_slot_reset_end_flow_wait_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    handler_va = TARGETS["slot_reset_end_flow_wait_handler_b0ccc0"]
    probe_va = TARGETS["slot_reset_end_flow_wait_probe_eb5530"]
    reset_va = TARGETS["slot_reset_end_flow_reset_67ae90"]
    handler_range = find_pdata_range_for_pc(data, image_base, sections, handler_va)
    probe_range = find_pdata_range_for_pc(data, image_base, sections, probe_va)
    handler_begin = int(str(handler_range.get("begin_va")), 16) if handler_range.get("begin_va") else handler_va
    handler_end = int(str(handler_range.get("end_va")), 16) if handler_range.get("end_va") else handler_va + 0xA2
    probe_begin = int(str(probe_range.get("begin_va")), 16) if probe_range.get("begin_va") else probe_va
    probe_end = int(str(probe_range.get("end_va")), 16) if probe_range.get("end_va") else probe_va + 0x33
    handler_blob = read_bytes(data, image_base, sections, handler_begin, max(0, handler_end - handler_begin))
    probe_blob = read_bytes(data, image_base, sections, probe_begin, max(0, probe_end - probe_begin))
    reset_blob = read_bytes(data, image_base, sections, reset_va, 0x0E)
    called_targets = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if handler_begin <= int(str(ref.get("source_va")), 16) < handler_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in [
            "slot_reset_end_flow_wait_probe_eb5530",
            "slot_reset_end_flow_reset_67ae90",
            "slot_reset_end_flow_branch_e780",
            "slot_reset_end_flow_branch_e650",
            "slot_reset_set_state_helper_b0d960",
        ]
    }
    finish_gate_refs = [
        ref
        for ref in scan_rip_relative_refs_to_va(
            data, image_base, sections, TARGETS["slot_reset_finish_gate_global_43d856a0"]
        )
        if ref.get("function_begin_va") == "0x140b0ccc0"
    ]
    end_flow_branch_gate_refs = [
        ref
        for ref in scan_rip_relative_refs_to_va(data, image_base, sections, 0x143D6F9C0)
        if ref.get("function_begin_va") == "0x140b0ccc0"
    ]
    return {
        "function_begin_va": handler_range.get("begin_va"),
        "function_end_va": handler_range.get("end_va"),
        "probe_function_begin_va": probe_range.get("begin_va"),
        "probe_function_end_va": probe_range.get("end_va"),
        "called_targets": called_targets,
        "finish_gate_refs": finish_gate_refs,
        "end_flow_branch_gate_refs": end_flow_branch_gate_refs,
        "captures_owner_rbx": b"\x48\x8b\xd9" in handler_blob,
        "sets_global_job_active_flag_6b0": b"\xc6\x80\xb0\x06\x00\x00\x01" in handler_blob,
        "probes_owner_c0": b"\x48\x8d\x8b\xc0\x00\x00\x00" in handler_blob
        and called_targets.get("slot_reset_end_flow_wait_probe_eb5530") == ["0x140b0cd11"],
        "probe_success_calls_reset_with_zero": b"\x33\xc9" in handler_blob
        and called_targets.get("slot_reset_end_flow_reset_67ae90") == ["0x140b0cd1c"],
        "branches_on_end_flow_gate": bool(end_flow_branch_gate_refs)
        and called_targets.get("slot_reset_end_flow_branch_e780") == ["0x140b0cd32"]
        and called_targets.get("slot_reset_end_flow_branch_e650") == ["0x140b0cd3c"],
        "branch_gate_global_va": "0x143d6f9c0" if end_flow_branch_gate_refs else None,
        "branch_gate_zero_jumps_e650": b"\x74\x0a" in handler_blob
        and called_targets.get("slot_reset_end_flow_branch_e650") == ["0x140b0cd3c"],
        "branch_gate_nonzero_jumps_e780": called_targets.get("slot_reset_end_flow_branch_e780") == ["0x140b0cd32"],
        "probe_success_branches_before_finish_gate": bool(
            called_targets.get("slot_reset_end_flow_branch_e780")
            and finish_gate_refs
            and int(called_targets["slot_reset_end_flow_branch_e780"][0], 16) < int(str(finish_gate_refs[0].get("source_va")), 16)
        ),
        "checks_finish_gate_before_state11": any(ref.get("source_va") == "0x140b0cd41" for ref in finish_gate_refs),
        "sets_state_11_via_helper": b"\xba\x0b\x00\x00\x00" in handler_blob
        and called_targets.get("slot_reset_set_state_helper_b0d960") == ["0x140b0cd57"],
        "returns_without_state_change_when_finish_gate_clear": b"\x74\x12" in handler_blob
        and b"\x48\x83\xc4\x20\x5b\xc3" in handler_blob,
        "probe_range_mapped": probe_range.get("begin_va") == "0x140eb5530"
        and probe_range.get("end_va") == "0x140eb5563",
        "probe_reads_owner_c0_plus8": b"\x48\x8b\x49\x08" in probe_blob,
        "probe_null_returns_true": b"\xb3\x01" in probe_blob
        and b"\x48\x85\xc9" in probe_blob
        and b"\x0f\xb6\xc3" in probe_blob,
        "probe_nonnull_calls_virtual_slot20": b"\x48\x8b\x01" in probe_blob
        and b"\xff\x50\x20" in probe_blob,
        "probe_returns_virtual_truthiness": b"\x84\xc0" in probe_blob
        and b"\x0f\x44\xca" in probe_blob
        and b"\x0f\xb6\xc1" in probe_blob,
        "reset_sets_game_man_b5e_from_cl": reset_blob.startswith(b"\x48\x8b\x05")
        and b"\x88\x88\x5e\x0b\x00\x00" in reset_blob
        and reset_blob.endswith(b"\xc3"),
        "reset_bytes_hex": reset_blob.hex(),
        "probe_bytes_hex": probe_blob.hex(),
        "bytes_hex": handler_blob.hex(),
    }


def read_slot_reset_end_flow_tail_branch_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    by_target = {addr: name for name, addr in TARGETS.items()}
    branch_targets = {
        "branch_e650": TARGETS["slot_reset_end_flow_branch_e650"],
        "branch_e780": TARGETS["slot_reset_end_flow_branch_e780"],
    }
    tracked_targets = [
        "slot_reset_end_flow_reset_67ae90",
        "slot_reset_set_state_helper_b0d960",
        "slot_reset_tail_counter_source_d298c0",
        "slot_reset_tail_calc_bc_29a950",
        "slot_reset_tail_mode_start_255930",
        "slot_reset_tail_mode_cleanup_2609a0",
        "slot_reset_tail_mode_set_5d18f0",
        "slot_reset_tail_mode_finish_6e9250",
        "slot_reset_tail_game_man_counter_reset_67a730",
        "slot_reset_tail_selected_value_set_67ac60",
        "slot_reset_tail_after_selected_value_67aac0",
        "slot_reset_tail_post_cleanup_1ca1c0",
        "slot_reset_tail_calc_bc_720260",
    ]
    branches: dict[str, Any] = {}
    for branch_name, branch_va in branch_targets.items():
        branch_range = find_pdata_range_for_pc(data, image_base, sections, branch_va)
        branch_begin = int(str(branch_range.get("begin_va")), 16) if branch_range.get("begin_va") else branch_va
        branch_end = int(str(branch_range.get("end_va")), 16) if branch_range.get("end_va") else branch_va + 0x120
        blob = read_bytes(data, image_base, sections, branch_begin, max(0, branch_end - branch_begin))
        calls_by_target = {
            target_name: sorted(
                [
                    str(ref.get("source_va"))
                    for ref in rel32_refs.get(target_name, [])
                    if branch_begin <= int(str(ref.get("source_va")), 16) < branch_end
                ],
                key=lambda value: int(value, 16),
            )
            for target_name in tracked_targets
        }
        rel_transfers: list[dict[str, Any]] = []
        for index in range(max(0, len(blob) - 5)):
            if blob[index] not in (0xE8, 0xE9):
                continue
            source_va = branch_begin + index
            target_va = source_va + 5 + struct.unpack_from("<i", blob, index + 1)[0]
            rel_transfers.append(
                {
                    "kind": "call" if blob[index] == 0xE8 else "jmp",
                    "source_va": f"0x{source_va:x}",
                    "target_va": f"0x{target_va:x}",
                    "target_name": by_target.get(target_va),
                }
            )
        branches[branch_name] = {
            "function_begin_va": branch_range.get("begin_va"),
            "function_end_va": branch_range.get("end_va"),
            "calls_by_target": calls_by_target,
            "rel_transfers": rel_transfers,
            "sets_owner_3e1_active": b"\xc6\x81\xe1\x03\x00\x00\x01" in blob,
            "sets_owner_3e0_complete": b"\xc6\x83\xe0\x03\x00\x00\x01" in blob,
            "sets_state_5_via_helper": b"\xba\x05\x00\x00\x00" in blob
            and bool(calls_by_target.get("slot_reset_set_state_helper_b0d960")),
            "writes_owner_bc_from_eax": b"\x89\x87\xbc\x00\x00\x00" in blob
            or b"\x89\x83\xbc\x00\x00\x00" in blob,
            "reads_counter_d0": b"\x44\x8b\x92\xd0\x00\x00\x00" in blob,
            "reads_selected_value_d4": b"\x8b\x81\xd4\x00\x00\x00" in blob,
            "caps_counter_120_to_270f": b"\x81\xf9\x0f\x27\x00\x00" in blob
            and b"\x89\x82\x20\x01\x00\x00" in blob,
            "passes_one_to_selected_value_set": b"\xb2\x01" in blob
            and bool(calls_by_target.get("slot_reset_tail_selected_value_set_67ac60")),
            "passes_one_to_b5e_setter": b"\xb1\x01" in blob
            and calls_by_target.get("slot_reset_end_flow_reset_67ae90") == ["0x140b0e77b"],
            "passes_zero_to_selected_value_set": b"\xc7\x44\x24\x30\x00\x00\x00\x00" in blob
            and bool(calls_by_target.get("slot_reset_tail_selected_value_set_67ac60")),
            "calls_counter_reset_before_selected_value_set": bool(
                calls_by_target.get("slot_reset_tail_game_man_counter_reset_67a730")
                and calls_by_target.get("slot_reset_tail_selected_value_set_67ac60")
                and int(calls_by_target["slot_reset_tail_game_man_counter_reset_67a730"][0], 16)
                < int(calls_by_target["slot_reset_tail_selected_value_set_67ac60"][0], 16)
            ),
            "does_not_call_b5e_setter": not bool(calls_by_target.get("slot_reset_end_flow_reset_67ae90")),
            "bytes_hex": blob.hex(),
        }
    return branches


def read_slot_reset_selected_value_field_access_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    patterns = {
        "ac0_global_slot_read": b"\x8b\x80\xc0\x0a\x00\x00",
        "b60_selected_value_write": b"\x89\x81\x60\x0b\x00\x00",
        "b5f_selected_value_flag_write": b"\x88\x91\x5f\x0b\x00\x00",
        "b5f_selected_value_flag_read": b"\x0f\xb6\x80\x5f\x0b\x00\x00",
        "b28_pair_normalization_gate_read": b"\x80\xb9\x28\x0b\x00\x00\x00",
        "bcc_validate_ready_write_one": b"\xc6\x81\xcc\x0b\x00\x00\x01",
        "bcc_validate_ready_write_zero": b"\xc6\x80\xcc\x0b\x00\x00\x00",
        "bcd_validate_12d_write": b"\x88\x88\xcd\x0b\x00\x00",
        "bce_validate_12e_write": b"\x88\x81\xce\x0b\x00\x00",
    }
    access_rows: dict[str, list[dict[str, Any]]] = {}
    for name, needle in patterns.items():
        rows: list[dict[str, Any]] = []
        cursor = 0
        while True:
            index = text_data.find(needle, cursor)
            if index < 0:
                break
            source_va = text_va + index
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            rows.append(
                {
                    "source_va": f"0x{source_va:x}",
                    "source_rva": f"0x{source_va - image_base:x}",
                    "function_begin_va": function_range.get("begin_va"),
                    "function_end_va": function_range.get("end_va"),
                    "bytes_hex": needle.hex(),
                }
            )
            cursor = index + 1
        access_rows[name] = rows
    get_slot_call_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_play_game_save_slot_get_678ca0", [])],
        key=lambda value: int(value, 16),
    )
    b5f_getter_call_sources = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_selected_value_b5f_getter_67a070", [])],
        key=lambda value: int(value, 16),
    )
    return {
        "access_rows": access_rows,
        "get_slot_call_sources": get_slot_call_sources,
        "b5f_getter_call_sources": b5f_getter_call_sources,
    }


def read_slot_reset_selected_value_caller_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    tracked_targets = [
        "slot_reset_play_game_save_slot_get_678ca0",
        "slot_reset_selected_value_b5f_getter_67a070",
        "slot_reset_selected_value_snapshot_679450",
        "slot_reset_selected_value_fallback_679280",
        "slot_reset_selected_value_mode_check_67a1e0",
        "slot_reset_selected_value_mode_value_679720",
        "slot_reset_selected_value_set_save_slot_wrapper_67a820",
        "slot_reset_end_flow_reset_67ae90",
        "slot_reset_play_game_submit_load_pair_67abd0",
        "post_requested_slot_67a320",
    ]
    caller_pcs = {
        "ac0_ui_store_9a9272": 0x1409A9272,
        "ac0_title_submit_aecf68": 0x140AECF68,
        "ac0_playgame_b0d690": 0x140B0D690,
        "b5f_load_selection_594918": 0x140594918,
    }
    contexts: dict[str, Any] = {}
    for name, pc in caller_pcs.items():
        function_range = find_pdata_range_for_pc(data, image_base, sections, pc)
        begin = int(str(function_range.get("begin_va")), 16) if function_range.get("begin_va") else max(pc - 0x80, image_base)
        end = int(str(function_range.get("end_va")), 16) if function_range.get("end_va") else pc + 0x120
        # Some pdata ranges include adjacent code; keep bounded local windows for signatures.
        window_begin = max(begin, pc - 0x90)
        window_end = min(end, pc + 0xE0)
        blob = read_bytes(data, image_base, sections, window_begin, max(0, window_end - window_begin))
        calls_by_target = {
            target_name: sorted(
                [
                    str(ref.get("source_va"))
                    for ref in rel32_refs.get(target_name, [])
                    if window_begin <= int(str(ref.get("source_va")), 16) < window_end
                ],
                key=lambda value: int(value, 16),
            )
            for target_name in tracked_targets
        }
        contexts[name] = {
            "function_begin_va": function_range.get("begin_va"),
            "function_end_va": function_range.get("end_va"),
            "window_begin_va": f"0x{window_begin:x}",
            "window_end_va": f"0x{window_end:x}",
            "calls_by_target": calls_by_target,
            "stores_nonnegative_ac0_to_global_1200": bool(
                calls_by_target.get("slot_reset_play_game_save_slot_get_678ca0")
                and b"\x85\xc0" in blob
                and b"\x78" in blob
                and b"\x89\x98\x00\x12\x00\x00" in blob
            ),
            "calls_post_requested_slot_after_ac0_gate": bool(
                calls_by_target.get("slot_reset_play_game_save_slot_get_678ca0")
                and calls_by_target.get("post_requested_slot_67a320") == ["0x140aecf8f"]
            ),
            "b5f_result_saved_in_ebx": bool(
                calls_by_target.get("slot_reset_selected_value_b5f_getter_67a070") == ["0x140594918"]
                and b"\x0f\xb6\xd8" in blob
            ),
            "b5f_true_sets_slot_zero": bool(
                "0x140594984" in calls_by_target.get("slot_reset_selected_value_set_save_slot_wrapper_67a820", [])
                and b"\xc7\x45\x28\x00\x00\x00\x00" in blob
                and b"\x84\xdb" in blob
                and b"\x74" in blob
            ),
            "b5f_true_sets_b5e_one": bool(
                calls_by_target.get("slot_reset_end_flow_reset_67ae90") == ["0x14059498b"]
                and b"\xb1\x01" in blob
            ),
            "always_normalizes_via_pair_helper": calls_by_target.get("slot_reset_play_game_submit_load_pair_67abd0") == [
                "0x1405949da"
            ],
            "bytes_hex": blob.hex(),
        }
    return contexts


def read_slot_reset_selected_value_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    get_va = TARGETS["slot_reset_play_game_save_slot_get_678ca0"]
    set_va = TARGETS["slot_reset_tail_selected_value_set_67ac60"]
    validate_va = TARGETS["slot_reset_play_game_submit_selected_value_validate_67aec0"]
    pair_va = TARGETS["slot_reset_play_game_submit_load_pair_67abd0"]
    get_blob = read_bytes(data, image_base, sections, get_va, 0x0E)
    set_blob = read_bytes(data, image_base, sections, set_va, 0x16)
    validate_range = find_pdata_range_for_pc(data, image_base, sections, validate_va)
    pair_range = find_pdata_range_for_pc(data, image_base, sections, pair_va)
    validate_begin = int(str(validate_range.get("begin_va")), 16) if validate_range.get("begin_va") else validate_va
    validate_end = int(str(validate_range.get("end_va")), 16) if validate_range.get("end_va") else validate_va + 0x9A
    pair_begin = int(str(pair_range.get("begin_va")), 16) if pair_range.get("begin_va") else pair_va
    pair_end = int(str(pair_range.get("end_va")), 16) if pair_range.get("end_va") else pair_va + 0x54
    validate_blob = read_bytes(data, image_base, sections, validate_begin, max(0, validate_end - validate_begin))
    pair_blob = read_bytes(data, image_base, sections, pair_begin, max(0, pair_end - pair_begin))
    tracked_targets = [
        "slot_reset_play_game_submit_validate_prepare_67f300",
        "slot_reset_play_game_submit_validate_probe_67d260",
        "slot_reset_play_game_submit_load_pair_convert_6783d0",
    ]
    validate_calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if validate_begin <= int(str(ref.get("source_va")), 16) < validate_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in tracked_targets
    }
    pair_calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if pair_begin <= int(str(ref.get("source_va")), 16) < pair_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in tracked_targets
    }
    return {
        "get_ac0_reads_global_slot": get_blob.startswith(b"\x48\x8b\x05")
        and b"\x8b\x80\xc0\x0a\x00\x00" in get_blob
        and get_blob.endswith(b"\xc3"),
        "set_b60_b5f_writes_value_and_flag": set_blob.startswith(b"\x8b\x01\x48\x8b\x0d")
        and b"\x89\x81\x60\x0b\x00\x00" in set_blob
        and b"\x88\x91\x5f\x0b\x00\x00" in set_blob
        and set_blob.endswith(b"\xc3"),
        "validate_function_begin_va": validate_range.get("begin_va"),
        "validate_function_end_va": validate_range.get("end_va"),
        "pair_function_begin_va": pair_range.get("begin_va"),
        "pair_function_end_va": pair_range.get("end_va"),
        "validate_calls_by_target": validate_calls_by_target,
        "pair_calls_by_target": pair_calls_by_target,
        "validate_checks_required_globals": validate_blob.count(b"\x48\x83\x3d") >= 2 and b"\x74" in validate_blob,
        "validate_copies_input_value_to_stack": b"\x8b\x01\x89\x44\x24\x48" in validate_blob,
        "validate_prepares_selected_value": validate_calls_by_target.get("slot_reset_play_game_submit_validate_prepare_67f300") == [
            "0x14067aef6"
        ],
        "validate_queries_12d_12e_flags": bool(
            validate_calls_by_target.get("slot_reset_play_game_submit_validate_probe_67d260")
            == ["0x14067af14", "0x14067af38"]
            and b"\xc7\x44\x24\x50\x2d\x01\x00\x00" in validate_blob
            and b"\xc7\x44\x24\x58\x2e\x01\x00\x00" in validate_blob
        ),
        "validate_sets_bcd_bce_and_bcc": bool(
            b"\x88\x88\xcd\x0b\x00\x00" in validate_blob
            and b"\x88\x81\xce\x0b\x00\x00" in validate_blob
            and b"\xc6\x81\xcc\x0b\x00\x00\x01" in validate_blob
        ),
        "pair_copies_input_to_output": b"\x8b\x02\x48\x8b\xd9\x89\x01" in pair_blob,
        "pair_requires_b28_clear_and_slot_not_minus_one": b"\x80\xb9\x28\x0b\x00\x00\x00" in pair_blob
        and b"\x83\x3a\xff" in pair_blob,
        "pair_rewrites_special_slot_range": bool(
            b"\x0f\xb6\x43\x03" in pair_blob
            and b"\x83\xe8\x32" in pair_blob
            and b"\x83\xf8\x26" in pair_blob
            and pair_calls_by_target.get("slot_reset_play_game_submit_load_pair_convert_6783d0") == ["0x14067ac06"]
        ),
        "pair_stores_output_to_game_man_14": b"\x89\x01" in pair_blob
        and b"\x89\x41\x14" in pair_blob,
        "get_bytes_hex": get_blob.hex(),
        "set_bytes_hex": set_blob.hex(),
        "validate_bytes_hex": validate_blob.hex(),
        "pair_bytes_hex": pair_blob.hex(),
    }


def read_slot_reset_play_game_submit_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    submit_va = TARGETS["slot_reset_play_game_submit_aebdc0"]
    submit_range = find_pdata_range_for_pc(data, image_base, sections, submit_va)
    submit_begin = int(str(submit_range.get("begin_va")), 16) if submit_range.get("begin_va") else submit_va
    submit_end = int(str(submit_range.get("end_va")), 16) if submit_range.get("end_va") else submit_va + 0x1CB
    blob = read_bytes(data, image_base, sections, submit_begin, max(0, submit_end - submit_begin))
    tracked_targets = [
        "slot_reset_play_game_submit_selected_value_validate_67aec0",
        "slot_reset_play_game_submit_load_pair_67abd0",
        "slot_reset_play_game_submit_check_720210",
        "slot_reset_play_game_submit_copy_aea780",
        "slot_reset_play_game_submit_vector_grow_218c70",
    ]
    calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if submit_begin <= int(str(ref.get("source_va")), 16) < submit_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in tracked_targets
    }
    return {
        "function_begin_va": submit_range.get("begin_va"),
        "function_end_va": submit_range.get("end_va"),
        "calls_by_target": calls_by_target,
        "captures_rcx_owner_job_rsi": b"\x48\x8b\xf1" in blob,
        "captures_rdx_slot_ptr_rdi": b"\x48\x8b\xfa" in blob,
        "captures_r9_payload_rbp": b"\x49\x8b\xe9" in blob,
        "stores_r8d_stack_arg": b"\x44\x89\x44\x24\x18" in blob,
        "returns_early_when_slot_minus_one": b"\x83\xcb\xff" in blob
        and b"\x39\x1f" in blob
        and b"\x0f\x84" in blob,
        "sets_owner_job_d8_active": b"\xc7\x86\xd8\x00\x00\x00\x01\x00\x00\x00" in blob,
        "copies_requested_slot_to_stack_for_helpers": b"\x8b\x07\x89\x44\x24\x68" in blob,
        "calls_selected_value_validate_then_load_pair": bool(
            calls_by_target.get("slot_reset_play_game_submit_selected_value_validate_67aec0") == ["0x140aebe4a"]
            and calls_by_target.get("slot_reset_play_game_submit_load_pair_67abd0") == ["0x140aebe69"]
        ),
        "stores_load_pair_to_owner_job_100_104": bool(
            b"\x8b\x4c\x24\x20\x89\x8e\x00\x01\x00\x00" in blob
            and b"\x0f\x4f\x5c\x24\x70" in blob
            and b"\x89\x9e\x04\x01\x00\x00" in blob
        ),
        "checks_high_word_and_optionally_copies_global": bool(
            calls_by_target.get("slot_reset_play_game_submit_check_720210") == ["0x140aebe93"]
            and calls_by_target.get("slot_reset_play_game_submit_copy_aea780") == ["0x140aebeaa"]
        ),
        "appends_payload_vector_to_owner_job_b35f0": bool(
            b"\x48\x81\xc6\xf0\x35\x0b\x00" in blob
            and b"\x8b\x3b\x89\x7c\x24\x68" in blob
            and b"\x48\x83\x46\x10\x04\x48\x83\xc3\x04\x48\xff\xc5" in blob
        ),
        "grows_owner_job_vector_when_full": calls_by_target.get("slot_reset_play_game_submit_vector_grow_218c70") == [
            "0x140aebf2c",
            "0x140aebf53",
        ],
        "bytes_hex": blob.hex(),
    }


def read_slot_reset_play_game_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    handler_va = TARGETS["slot_reset_play_game_handler_b0d5b0"]
    tail_va = TARGETS["slot_reset_play_game_tail_b0d850"]
    handler_range = find_pdata_range_for_pc(data, image_base, sections, handler_va)
    tail_range = find_pdata_range_for_pc(data, image_base, sections, tail_va)
    handler_begin = int(str(handler_range.get("begin_va")), 16) if handler_range.get("begin_va") else handler_va
    handler_end = int(str(handler_range.get("end_va")), 16) if handler_range.get("end_va") else handler_va + 0x282
    tail_begin = int(str(tail_range.get("begin_va")), 16) if tail_range.get("begin_va") else tail_va
    tail_end = int(str(tail_range.get("end_va")), 16) if tail_range.get("end_va") else tail_va + 0x109
    handler_blob = read_bytes(data, image_base, sections, handler_begin, max(0, handler_end - handler_begin))
    tail_blob = read_bytes(data, image_base, sections, tail_begin, max(0, tail_end - tail_begin))
    tracked_targets = [
        "slot_reset_play_game_tail_b0d850",
        "slot_reset_play_game_render_start_e5f8f0",
        "slot_reset_play_game_render_finish_e5f7f0",
        "slot_reset_play_game_save_slot_get_678ca0",
        "slot_reset_play_game_global_obj_256360",
        "slot_reset_play_game_prepare_aea590",
        "slot_reset_play_game_submit_aebdc0",
        "slot_reset_play_game_consume_owner300_ca89e0",
        "slot_reset_play_game_probe_a4cf70",
        "slot_reset_play_game_probe_a4cf90",
        "slot_reset_play_game_action_a4d7d0",
        "slot_reset_play_game_status_256590",
        "slot_reset_play_game_notify_cc7930",
    ]
    handler_calls_by_target = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if handler_begin <= int(str(ref.get("source_va")), 16) < handler_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in tracked_targets
    }
    return {
        "function_begin_va": handler_range.get("begin_va"),
        "function_end_va": handler_range.get("end_va"),
        "tail_function_begin_va": tail_range.get("begin_va"),
        "tail_function_end_va": tail_range.get("end_va"),
        "handler_calls_by_target": handler_calls_by_target,
        "sets_global_job_active_flag_6b0": b"\xc6\x80\xb0\x06\x00\x00\x01" in handler_blob,
        "calls_render_start_with_zero_arg": b"\x33\xd2" in handler_blob
        and handler_calls_by_target.get("slot_reset_play_game_render_start_e5f8f0") == ["0x140b0d64c"],
        "calls_render_finish": handler_calls_by_target.get("slot_reset_play_game_render_finish_e5f7f0") == [
            "0x140b0d68b"
        ],
        "gets_save_slot_and_stores_nonnegative_to_global_1200": bool(
            handler_calls_by_target.get("slot_reset_play_game_save_slot_get_678ca0") == ["0x140b0d690"]
            and handler_calls_by_target.get("slot_reset_play_game_global_obj_256360") == ["0x140b0d69b"]
            and b"\x85\xc0" in handler_blob
            and b"\x78\x0b" in handler_blob
            and b"\x89\x98\x00\x12\x00\x00" in handler_blob
        ),
        "prepares_stack_payload_from_arg_flag": handler_calls_by_target.get("slot_reset_play_game_prepare_aea590") == [
            "0x140b0d6b0"
        ]
        and b"\x0f\xb6\x54\x24\x60" in handler_blob,
        "submits_owner_bc_and_owner_2e8_job": handler_calls_by_target.get("slot_reset_play_game_submit_aebdc0") == [
            "0x140b0d6df"
        ]
        and b"\x8b\x87\xbc\x00\x00\x00" in handler_blob
        and b"\x48\x8b\x8f\xe8\x02\x00\x00" in handler_blob,
        "cleans_submitted_payload_virtual_slot68": b"\xff\x50\x68" in handler_blob
        and (b"\x0f\x11\x44\x24\x38" in handler_blob or b"\xf3\x0f\x7f\x44\x24\x38" in handler_blob),
        "consumes_owner_3e1_flag_and_clears_it": bool(
            handler_calls_by_target.get("slot_reset_play_game_consume_owner300_ca89e0") == ["0x140b0d71c"]
            and b"\x80\xbf\xe1\x03\x00\x00\x00" in handler_blob
            and b"\x48\x8d\x8f\x00\x03\x00\x00" in handler_blob
            and b"\xc6\x87\xe1\x03\x00\x00\x00" in handler_blob
        ),
        "checks_global_menu_object_b0_probes": bool(
            handler_calls_by_target.get("slot_reset_play_game_probe_a4cf70") == ["0x140b0d748"]
            and handler_calls_by_target.get("slot_reset_play_game_probe_a4cf90") == ["0x140b0d799"]
            and handler_calls_by_target.get("slot_reset_play_game_action_a4d7d0") == ["0x140b0d7fc"]
        ),
        "status_notify_path_mapped": bool(
            handler_calls_by_target.get("slot_reset_play_game_status_256590") == ["0x140b0d801"]
            and handler_calls_by_target.get("slot_reset_play_game_notify_cc7930") == ["0x140b0d81b"]
            and b"\x0f\xb6\x50\x21" in handler_blob
        ),
        "tail_jumps_to_state_increment_helper": handler_calls_by_target.get("slot_reset_play_game_tail_b0d850") == [
            "0x140b0d82d"
        ],
        "tail_increments_owner_4c_and_bounds_owner_48": b"\xff\x41\x4c" in tail_blob
        and b"\x8b\x41\x48" in tail_blob
        and b"\x83\xf8\x0e" in tail_blob,
        "handler_bytes_hex": handler_blob.hex(),
        "tail_bytes_hex": tail_blob.hex(),
    }


def read_slot_reset_title_queue_state_table_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
) -> dict[str, Any]:
    init_va = TARGETS["slot_reset_title_queue_state_table_init_0a4c90"]
    table_va = TARGETS["slot_reset_title_queue_state_table_global_43d71340"]
    init_range = find_pdata_range_for_pc(data, image_base, sections, init_va)
    init_begin = int(str(init_range.get("begin_va")), 16) if init_range.get("begin_va") else init_va
    init_end = int(str(init_range.get("end_va")), 16) if init_range.get("end_va") else init_va + 0xC3
    blob = read_bytes(data, image_base, sections, init_begin, max(0, init_end - init_begin))
    stores: list[dict[str, Any]] = []
    for index in range(max(0, len(blob) - 14)):
        if blob[index : index + 3] not in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
            continue
        if blob[index + 7 : index + 10] != b"\x48\x89\x05":
            continue
        value_disp = struct.unpack_from("<i", blob, index + 3)[0]
        store_disp = struct.unpack_from("<i", blob, index + 10)[0]
        source_va = init_begin + index
        value_va = source_va + 7 + value_disp
        store_va = source_va + 14 + store_disp
        if not (table_va <= store_va < table_va + 0x60):
            continue
        offset = store_va - table_va
        entry_index = offset // 16
        entry_slot = "handler" if offset % 16 == 0 else "label"
        row: dict[str, Any] = {
            "source_va": f"0x{source_va:x}",
            "store_va": f"0x{store_va:x}",
            "store_offset": f"0x{offset:x}",
            "entry_index": entry_index,
            "entry_slot": entry_slot,
            "value_va": f"0x{value_va:x}",
        }
        if entry_slot == "label":
            row["label_text"] = read_utf16z(data, image_base, sections, value_va)
        stores.append(row)
    entries: list[dict[str, Any]] = []
    for entry_index in sorted({int(row["entry_index"]) for row in stores}):
        handler = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "handler"), None)
        label = next((row for row in stores if row["entry_index"] == entry_index and row["entry_slot"] == "label"), None)
        entries.append(
            {
                "entry_index": entry_index,
                "handler_va": handler.get("value_va") if handler else None,
                "handler_source_va": handler.get("source_va") if handler else None,
                "label_va": label.get("value_va") if label else None,
                "label_source_va": label.get("source_va") if label else None,
                "label_text": label.get("label_text") if label else None,
            }
        )
    handler_ranges = {
        str(entry.get("handler_va")): find_pdata_range_for_pc(data, image_base, sections, int(str(entry.get("handler_va")), 16))
        for entry in entries
        if entry.get("handler_va")
    }
    return {
        "function_begin_va": init_range.get("begin_va"),
        "function_end_va": init_range.get("end_va"),
        "table_base_va": f"0x{table_va:x}",
        "zeroes_table_base": function_has_rip_lea_any_reg_to(data, image_base, sections, init_begin, table_va, max(0, init_end - init_begin)),
        "zero_size_bytes": 0x60 if b"\x44\x8d\x42\x60" in blob else None,
        "stores": stores,
        "entries": entries,
        "handler_ranges": handler_ranges,
        "bytes_hex": blob.hex(),
    }


def read_slot_reset_title_queue_producer_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    seed_va = TARGETS["slot_reset_title_queue_seed_b0a4a0"]
    pump_va = TARGETS["slot_reset_title_queue_pump_b0a5e0"]
    advance_va = TARGETS["slot_reset_title_queue_advance_b0a980"]
    set_state_va = TARGETS["slot_reset_title_queue_state_set_b0aa90"]
    seed_range = find_pdata_range_for_pc(data, image_base, sections, seed_va)
    pump_range = find_pdata_range_for_pc(data, image_base, sections, pump_va)
    advance_range = find_pdata_range_for_pc(data, image_base, sections, advance_va)
    set_state_range = find_pdata_range_for_pc(data, image_base, sections, set_state_va)
    seed_begin = int(str(seed_range.get("begin_va")), 16) if seed_range.get("begin_va") else seed_va
    seed_end = int(str(seed_range.get("end_va")), 16) if seed_range.get("end_va") else seed_va + 0x13B
    pump_begin = int(str(pump_range.get("begin_va")), 16) if pump_range.get("begin_va") else pump_va
    pump_end = int(str(pump_range.get("end_va")), 16) if pump_range.get("end_va") else pump_va + 0x39F
    advance_begin = int(str(advance_range.get("begin_va")), 16) if advance_range.get("begin_va") else advance_va
    advance_end = int(str(advance_range.get("end_va")), 16) if advance_range.get("end_va") else advance_va + 0x109
    set_state_begin = int(str(set_state_range.get("begin_va")), 16) if set_state_range.get("begin_va") else set_state_va
    set_state_end = int(str(set_state_range.get("end_va")), 16) if set_state_range.get("end_va") else set_state_va + 0x109
    seed_blob = read_bytes(data, image_base, sections, seed_begin, max(0, seed_end - seed_begin))
    pump_blob = read_bytes(data, image_base, sections, pump_begin, max(0, pump_end - pump_begin))
    advance_blob = read_bytes(data, image_base, sections, advance_begin, max(0, advance_end - advance_begin))
    set_state_blob = read_bytes(data, image_base, sections, set_state_begin, max(0, set_state_end - set_state_begin))

    def calls_in_range(target_name: str, begin: int, end: int) -> list[str]:
        return sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if begin <= int(str(ref.get("source_va")), 16) < end
            ],
            key=lambda value: int(value, 16),
        )

    seed_calls = {
        target_name: calls_in_range(target_name, seed_begin, seed_end)
        for target_name in [
            "slot_reset_title_queue_source_81f7e0",
            "task_enqueue_7a7b60",
            "slot_reset_branch_gate_submit_attach_7a9460",
            "slot_reset_title_queue_advance_b0a980",
        ]
    }
    pump_calls = {
        target_name: calls_in_range(target_name, pump_begin, pump_end)
        for target_name in [
            "slot_reset_menu_job_wait_task_submit_733f20",
            "slot_reset_menu_job_wait_queue_7a9600",
            "slot_reset_title_queue_advance_b0a980",
            "slot_reset_title_queue_state_set_b0aa90",
            "slot_reset_title_queue_stream_validate_71fd60",
            "slot_reset_title_queue_take_7a9560",
        ]
    }
    state_set_callers = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_title_queue_state_set_b0aa90", [])],
        key=lambda value: int(value, 16),
    )
    advance_callers = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_title_queue_advance_b0a980", [])],
        key=lambda value: int(value, 16),
    )
    return {
        "seed_begin_va": seed_range.get("begin_va"),
        "seed_end_va": seed_range.get("end_va"),
        "pump_begin_va": pump_range.get("begin_va"),
        "pump_end_va": pump_range.get("end_va"),
        "advance_begin_va": advance_range.get("begin_va"),
        "advance_end_va": advance_range.get("end_va"),
        "set_state_begin_va": set_state_range.get("begin_va"),
        "set_state_end_va": set_state_range.get("end_va"),
        "seed_calls_by_target": seed_calls,
        "pump_calls_by_target": pump_calls,
        "state_set_callers": state_set_callers,
        "advance_callers": advance_callers,
        "seed_builds_task_from_owner_d8": seed_calls.get("slot_reset_title_queue_source_81f7e0") == ["0x140b0a513"]
        and b"\x48\x8d\x97\xd8\x00\x00\x00" in seed_blob,
        "seed_enqueues_task_and_attaches_to_owner128": seed_calls.get("task_enqueue_7a7b60") == ["0x140b0a521"]
        and seed_calls.get("slot_reset_branch_gate_submit_attach_7a9460") == ["0x140b0a536"]
        and b"\x48\x8d\x8f\x28\x01\x00\x00" in seed_blob,
        "seed_resets_owner130_selection_and_advances": b"\xc7\x87\x30\x01\x00\x00\xff\xff\xff\xff" in seed_blob
        and seed_calls.get("slot_reset_title_queue_advance_b0a980") == ["0x140b0a5d6"],
        "pump_submits_owner_d8_and_queues_owner128": pump_calls.get("slot_reset_menu_job_wait_task_submit_733f20") == ["0x140b0a6a0"]
        and pump_calls.get("slot_reset_menu_job_wait_queue_7a9600") == ["0x140b0a6d2"]
        and b"\x48\x8d\x8b\xd8\x00\x00\x00" in pump_blob
        and b"\x48\x8d\x8b\x28\x01\x00\x00" in pump_blob,
        "pump_advances_when_owner128_empty": b"\x4c\x39\xa3\x28\x01\x00\x00\x75\x0d" in pump_blob
        and pump_calls.get("slot_reset_title_queue_advance_b0a980")
        and "0x140b0a6e6" in pump_calls.get("slot_reset_title_queue_advance_b0a980", []),
        "pump_sets_state5_on_gate_or_finish": pump_calls.get("slot_reset_title_queue_state_set_b0aa90") == [
            "0x140b0a77d",
            "0x140b0a7a8",
        ],
        "pump_parses_stream_selection_to_owner130": pump_calls.get("slot_reset_title_queue_stream_validate_71fd60") == [
            "0x140b0a8bc"
        ]
        and b"\x89\x83\x30\x01\x00\x00" in pump_blob,
        "pump_takes_owner128_queue_after_selection": pump_calls.get("slot_reset_title_queue_take_7a9560") == [
            "0x140b0a8d8"
        ]
        and b"\x48\x8d\x8b\x28\x01\x00\x00" in pump_blob,
        "pump_advances_after_selection_queue_take": "0x140b0a919" in pump_calls.get(
            "slot_reset_title_queue_advance_b0a980", []
        ),
        "advance_increments_owner_4c_and_bounds_owner_48": b"\xff\x41\x4c" in advance_blob
        and b"\x8b\x41\x48" in advance_blob
        and b"\x83\xf8\x07" in advance_blob,
        "advance_callers_cover_seed_pump_and_guard_paths": advance_callers
        == ["0x140b0a424", "0x140b0a492", "0x140b0a5d6", "0x140b0a6e6", "0x140b0a919"],
        "set_state_range_mapped": set_state_range.get("begin_va") == "0x140b0aa90"
        and set_state_range.get("end_va") == "0x140b0ab99",
        "set_state_writes_requested_state_to_owner_4c": b"\x89\x51\x4c" in set_state_blob,
        "set_state_validates_owner_48_bounds": b"\x8b\x41\x48\xff\xc0\x83\xf8\x07" in set_state_blob,
        "set_state_callers_are_ingame_gate_and_menu_loop": state_set_callers
        == ["0x140b0a1d8", "0x140b0a77d", "0x140b0a7a8"],
        "set_state_menu_loop_callers_use_state5": b"\xba\x05\x00\x00\x00\x48\x8b\xcb\xe8\x0e\x03\x00\x00" in pump_blob
        and b"\xba\x05\x00\x00\x00\x48\x8b\xcb\xe8\xe3\x02\x00\x00" in pump_blob,
        "set_state_ingame_gate_caller_uses_state0": "0x140b0a1d8" in state_set_callers,
        "seed_bytes_hex": seed_blob.hex(),
        "pump_bytes_hex": pump_blob.hex(),
        "advance_bytes_hex": advance_blob.hex(),
        "set_state_bytes_hex": set_state_blob.hex(),
    }


def read_slot_reset_menu_job_wait_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    handler_va = TARGETS["slot_reset_menu_job_wait_handler_b0d400"]
    set_state_va = TARGETS["slot_reset_set_state_helper_b0d960"]
    handler_range = find_pdata_range_for_pc(data, image_base, sections, handler_va)
    set_state_range = find_pdata_range_for_pc(data, image_base, sections, set_state_va)
    handler_begin = int(str(handler_range.get("begin_va")), 16) if handler_range.get("begin_va") else handler_va
    handler_end = int(str(handler_range.get("end_va")), 16) if handler_range.get("end_va") else handler_va + 0x149
    set_state_begin = int(str(set_state_range.get("begin_va")), 16) if set_state_range.get("begin_va") else set_state_va
    set_state_end = int(str(set_state_range.get("end_va")), 16) if set_state_range.get("end_va") else set_state_va + 0x109
    handler_blob = read_bytes(data, image_base, sections, handler_begin, max(0, handler_end - handler_begin))
    set_state_blob = read_bytes(data, image_base, sections, set_state_begin, max(0, set_state_end - set_state_begin))
    called_targets = {
        target_name: sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if handler_begin <= int(str(ref.get("source_va")), 16) < handler_end
            ],
            key=lambda value: int(value, 16),
        )
        for target_name in [
            "slot_reset_menu_job_wait_task_submit_733f20",
            "slot_reset_menu_job_wait_queue_7a9600",
            "slot_reset_menu_job_wait_global_toggle_7663c0",
            "slot_reset_set_state_helper_b0d960",
            "task_enqueue_7a7b60",
            "task_enqueue_link_7a7bb0",
        ]
    }
    set_state_callers = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_set_state_helper_b0d960", [])],
        key=lambda value: int(value, 16),
    )
    finish_gate_refs = scan_rip_relative_refs_to_va(
        data, image_base, sections, TARGETS["slot_reset_finish_gate_global_43d856a0"]
    )
    return {
        "menu_job_wait_begin_va": handler_range.get("begin_va"),
        "menu_job_wait_end_va": handler_range.get("end_va"),
        "set_state_begin_va": set_state_range.get("begin_va"),
        "set_state_end_va": set_state_range.get("end_va"),
        "called_targets": called_targets,
        "set_state_callers": set_state_callers,
        "finish_gate_refs": finish_gate_refs,
        "captures_owner_rbx_and_task_arg_rdi": b"\x48\x8b\xfa" in handler_blob
        and b"\x48\x8b\xd9" in handler_blob,
        "sets_global_job_active_flag_6b0": b"\xc6\x80\xb0\x06\x00\x00\x01" in handler_blob,
        "builds_timed_descriptor_from_arg_plus8": function_has_rip_lea_any_reg_to(
            data, image_base, sections, handler_begin, 0x1429C8E48, max(0, handler_end - handler_begin)
        )
        and function_has_rip_lea_any_reg_to(
            data, image_base, sections, handler_begin, 0x1429C8E58, max(0, handler_end - handler_begin)
        )
        and b"\xf3\x0f\x10\x47\x08" in handler_blob,
        "submits_owner_e0_timed_task": b"\x48\x8d\x8b\xe0\x00\x00\x00" in handler_blob
        and called_targets.get("slot_reset_menu_job_wait_task_submit_733f20") == ["0x140b0d4a8"],
        "calls_global_toggle_after_first_submit": called_targets.get("slot_reset_menu_job_wait_global_toggle_7663c0") == [
            "0x140b0d4e9"
        ],
        "queues_owner_130_timed_task": b"\x48\x8d\x8b\x30\x01\x00\x00" in handler_blob
        and called_targets.get("slot_reset_menu_job_wait_queue_7a9600") == ["0x140b0d521"],
        "submit_then_queue_order_mapped": called_targets.get("slot_reset_menu_job_wait_task_submit_733f20") == ["0x140b0d4a8"]
        and called_targets.get("slot_reset_menu_job_wait_global_toggle_7663c0") == ["0x140b0d4e9"]
        and called_targets.get("slot_reset_menu_job_wait_queue_7a9600") == ["0x140b0d521"],
        "reuses_frame_delta_descriptor_for_submit_and_queue": b"\xf3\x0f\x10\x47\x08" in handler_blob
        and handler_blob.count(b"\xf3\x0f\x10\x47\x08") >= 2
        and b"\x48\x8d\x35\xd5\xb9\xeb\x01" in handler_blob
        and b"\x48\x8d\x2d\xd1\xb9\xeb\x01" in handler_blob,
        "does_not_directly_enqueue_title_accept_payload": called_targets.get("task_enqueue_7a7b60") == []
        and called_targets.get("task_enqueue_link_7a7bb0") == [],
        "finish_gate_checked_only_after_owner130_queue": called_targets.get("slot_reset_menu_job_wait_queue_7a9600") == ["0x140b0d521"]
        and b"\xe8\xda\xc0\xc9\xff\x80\x3d\x73\x81\x27\x03\x00" in handler_blob,
        "conditionally_sets_state_11": b"\x80\x3d" in handler_blob
        and b"\xba\x0b\x00\x00\x00" in handler_blob
        and b"\x48\x8b\xcb" in handler_blob
        and called_targets.get("slot_reset_set_state_helper_b0d960") == ["0x140b0d537"],
        "set_state_helper_stores_edx_to_owner_4c": b"\x89\x51\x4c" in set_state_blob,
        "set_state_helper_validates_current_plus_one_le_0xe": b"\x8b\x41\x48\xff\xc0\x83\xf8\x0e" in set_state_blob,
        "set_state_helper_has_finish_callers": {"0x140b0cd57", "0x140b0d537"}.issubset(set(set_state_callers)),
        "finish_gate_refs_include_menu_job_wait": any(
            ref.get("source_va") == "0x140b0d526" and ref.get("function_begin_va") == "0x140b0d400"
            for ref in finish_gate_refs
        ),
        "finish_gate_refs_include_finish_handler": any(
            ref.get("source_va") == "0x140b0cd41"
            and ref.get("function_begin_va") in {"0x140b0cd70", "0x140b0ccc0"}
            for ref in finish_gate_refs
        ),
        "finish_gate_refs_include_save_request_gate_family": {
            "0x14067a3d4",
            "0x14067a3e4",
            "0x14067a509",
            "0x14067a66a",
        }.issubset({str(ref.get("source_va")) for ref in finish_gate_refs}),
        "finish_gate_ref_count": len(finish_gate_refs),
        "menu_job_wait_bytes_hex": handler_blob.hex(),
        "set_state_bytes_hex": set_state_blob.hex(),
    }


def read_slot_reset_dispatch_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    absolute_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    parent_va = TARGETS["slot_reset_parent_loop_b0bd60"]
    handler_va = TARGETS["slot_reset_handler_b0cd70"]
    parent_range = find_pdata_range_for_pc(data, image_base, sections, parent_va)
    handler_range = find_pdata_range_for_pc(data, image_base, sections, handler_va)
    parent_begin = int(str(parent_range.get("begin_va")), 16) if parent_range.get("begin_va") else parent_va
    parent_end = int(str(parent_range.get("end_va")), 16) if parent_range.get("end_va") else parent_va + 0x243
    handler_begin = int(str(handler_range.get("begin_va")), 16) if handler_range.get("begin_va") else handler_va
    handler_end = int(str(handler_range.get("end_va")), 16) if handler_range.get("end_va") else handler_va + 0x6E
    parent_blob = read_bytes(data, image_base, sections, parent_begin, max(0, parent_end - parent_begin))
    handler_blob = read_bytes(data, image_base, sections, handler_begin, max(0, handler_end - handler_begin))
    parent_vtable_contexts: list[dict[str, Any]] = []
    for ref in sorted(
        absolute_refs.get("slot_reset_parent_loop_b0bd60", []),
        key=lambda item: int(str(item.get("source_va")), 16),
    ):
        source_va = int(str(ref.get("source_va")), 16)
        base_va = source_va - 0x28
        parent_vtable_contexts.append(
            {
                "source_va": ref.get("source_va"),
                "section": ref.get("section"),
                "vtable_base_va": f"0x{base_va:x}",
                "parent_slot_offset": "0x28",
                "entries": read_qwords(data, image_base, sections, base_va, 12),
            }
        )
    return {
        "parent_function_begin_va": parent_range.get("begin_va"),
        "parent_function_end_va": parent_range.get("end_va"),
        "handler_function_begin_va": handler_range.get("begin_va"),
        "handler_function_end_va": handler_range.get("end_va"),
        "parent_ref_sources": sorted(
            [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_parent_loop_b0bd60", [])],
            key=lambda value: int(value, 16),
        ),
        "handler_ref_sources": sorted(
            [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_handler_b0cd70", [])],
            key=lambda value: int(value, 16),
        ),
        "parent_vtable_contexts": parent_vtable_contexts,
        "parent_captures_owner_rbx_and_arg_rsi": b"\x48\x8b\xf2" in parent_blob
        and b"\x48\x8b\xd9" in parent_blob,
        "parent_copies_state_4c_to_current_48": b"\x48\x63\x43\x4c\x89\x43\x48" in parent_blob,
        "parent_updates_label_a0_from_table_plus8": b"\x48\x8b\x43\x10" in parent_blob
        and b"\x48\x8b\x4c\xc8\x08" in parent_blob
        and b"\x48\x89\x8b\xa0\x00\x00\x00" in parent_blob,
        "parent_sets_default_label_for_minus_one_state": function_has_rip_lea_any_reg_to(
            data, image_base, sections, parent_begin, 0x143294DC0, max(0, parent_end - parent_begin)
        )
        and b"\x83\xf8\xff" in parent_blob
        and b"\x48\x89\xab\xa0\x00\x00\x00" in parent_blob,
        "parent_calls_owner_virtual_slot20_before_dispatch": b"\x48\x8b\x03" in parent_blob
        and b"\xff\x50\x20" in parent_blob,
        "parent_dispatches_state_table_entry": b"\x4c\x63\x43\x48" in parent_blob
        and b"\x4d\x03\xc0" in parent_blob
        and b"\x42\xff\x14\xc0" in parent_blob,
        "parent_refreshes_state_after_dispatch": parent_blob.count(b"\x48\x63\x43\x4c\x89\x43\x48") >= 2,
        "parent_loop_flag_and_cap_mapped": b"\xff\xc7" in parent_blob
        and b"\x44\x38\x73\x50" in parent_blob
        and b"\x81\xff\x80\x00\x00\x00" in parent_blob
        and b"\xc6\x43\x50\x00" in parent_blob,
        "parent_trace_maps_state_table_requested_current_label": b"\x48\x8b\x43\x10" in parent_blob
        and b"\x48\x63\x43\x4c" in parent_blob
        and b"\x89\x43\x48" in parent_blob
        and b"\x48\x8b\x4c\xc8\x08" in parent_blob
        and b"\x48\x89\x8b\xa0\x00\x00\x00" in parent_blob,
        "parent_trace_maps_loop_flags_and_counters": b"\x44\x38\x73\x50" in parent_blob
        and b"\x48\x8b\x4b\x60" in parent_blob
        and b"\x44\x38\x73\x69" in parent_blob
        and b"\x44\x38\xb3\xa8\x00\x00\x00" in parent_blob
        and b"\x8b\x83\xac\x00\x00\x00" in parent_blob,
        "parent_trace_dispatch_callsite_mapped": b"\x4c\x63\x43\x48" in parent_blob
        and b"\x4d\x03\xc0" in parent_blob
        and b"\x42\xff\x14\xc0" in parent_blob
        and b"\x48\x63\x43\x4c\x89\x43\x48" in parent_blob,
        "handler_increments_owner_b0_and_requires_gt_one": b"\xff\x81\xb0\x00\x00\x00" in handler_blob
        and b"\x83\xb9\xb0\x00\x00\x00\x01" in handler_blob
        and b"\x7e\x50" in handler_blob,
        "handler_passes_minus_one_to_set_save_slot": b"\x83\xc9\xff" in handler_blob
        and any(ref.get("source_va") == "0x140b0cd8b" for ref in rel32_refs.get("set_save_slot_67a810", [])),
        "handler_uses_global_after_reset": function_has_rip_lea_any_reg_to(
            data, image_base, sections, handler_begin, 0x143D5DF48, max(0, handler_end - handler_begin)
        )
        and b"\x48\x8b\x0d" in handler_blob,
        "handler_marks_owner_4c_minus_one_after_reset": b"\xc7\x43\x4c\xff\xff\xff\xff" in handler_blob,
        "handler_bytes_hex": handler_blob.hex(),
        "parent_bytes_hex": parent_blob.hex(),
    }


def read_slot_reset_title_parent_ctor_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, Any]:
    wrapper_va = TARGETS["slot_reset_title_parent_ctor_b0b020"]
    base_ctor_va = TARGETS["slot_reset_title_parent_base_ctor_b0b0d0"]
    title_ctor_va = TARGETS["slot_reset_title_step_ctor_b0b1c0"]
    state_table_va = TARGETS["slot_reset_state_table_global_43d71580"]
    simple_table_va = TARGETS["slot_reset_title_queue_state_table_global_43d71340"]
    wrapper_range = find_pdata_range_for_pc(data, image_base, sections, wrapper_va)
    base_ctor_range = find_pdata_range_for_pc(data, image_base, sections, base_ctor_va)
    title_ctor_range = find_pdata_range_for_pc(data, image_base, sections, title_ctor_va)
    wrapper_begin = int(str(wrapper_range.get("begin_va")), 16) if wrapper_range.get("begin_va") else wrapper_va
    wrapper_end = int(str(wrapper_range.get("end_va")), 16) if wrapper_range.get("end_va") else wrapper_va + 0xA6
    base_ctor_begin = int(str(base_ctor_range.get("begin_va")), 16) if base_ctor_range.get("begin_va") else base_ctor_va
    base_ctor_end = int(str(base_ctor_range.get("end_va")), 16) if base_ctor_range.get("end_va") else base_ctor_va + 0xE2
    title_ctor_begin = int(str(title_ctor_range.get("begin_va")), 16) if title_ctor_range.get("begin_va") else title_ctor_va
    title_ctor_end = int(str(title_ctor_range.get("end_va")), 16) if title_ctor_range.get("end_va") else title_ctor_va + 0x130
    wrapper_blob = read_bytes(data, image_base, sections, wrapper_begin, max(0, wrapper_end - wrapper_begin))
    base_ctor_blob = read_bytes(data, image_base, sections, base_ctor_begin, max(0, base_ctor_end - base_ctor_begin))
    title_ctor_blob = read_bytes(data, image_base, sections, title_ctor_begin, max(0, title_ctor_end - title_ctor_begin))

    def calls_in_range(target_name: str, begin: int, end: int) -> list[str]:
        return sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if begin <= int(str(ref.get("source_va")), 16) < end
            ],
            key=lambda value: int(value, 16),
        )

    title_table_refs = scan_rip_relative_refs_to_va(data, image_base, sections, state_table_va)
    simple_table_refs = scan_rip_relative_refs_to_va(data, image_base, sections, simple_table_va)
    title_final_vtable_base = 0x142B63BA0
    title_final_vtable_parent_slot = title_final_vtable_base + 0x28
    return {
        "wrapper_begin_va": wrapper_range.get("begin_va"),
        "wrapper_end_va": wrapper_range.get("end_va"),
        "base_ctor_begin_va": base_ctor_range.get("begin_va"),
        "base_ctor_end_va": base_ctor_range.get("end_va"),
        "title_ctor_begin_va": title_ctor_range.get("begin_va"),
        "title_ctor_end_va": title_ctor_range.get("end_va"),
        "title_table_refs": title_table_refs,
        "simple_table_refs": simple_table_refs,
        "simple_table_refs_are_initializer_only": len(simple_table_refs) == 2
        and all(ref.get("function_begin_va") == "0x1400a4c90" for ref in simple_table_refs),
        "title_table_refs_include_initializer_and_constructor": any(
            ref.get("source_va") == "0x1400a4f56" and ref.get("function_begin_va") == "0x1400a4f50"
            for ref in title_table_refs
        )
        and any(
            ref.get("source_va") == "0x140b0b1d8" and ref.get("function_begin_va") == "0x140b0b1c0"
            for ref in title_table_refs
        ),
        "title_ctor_final_vtable_parent_loop_slot28": read_qword_value(
            data, image_base, sections, title_final_vtable_parent_slot
        )
        == TARGETS["slot_reset_parent_loop_b0bd60"],
        "passive_trace_anchor_is_parent_loop_not_simple_table": len(simple_table_refs) == 2
        and all(ref.get("function_begin_va") == "0x1400a4c90" for ref in simple_table_refs)
        and any(ref.get("function_begin_va") == "0x140b0b1c0" for ref in title_table_refs)
        and read_qword_value(data, image_base, sections, title_final_vtable_parent_slot)
        == TARGETS["slot_reset_parent_loop_b0bd60"],
        "title_ctor_passes_title_table_to_wrapper": any(
            ref.get("source_va") == "0x140b0b1d8"
            and ref.get("instruction") == "lea_rdx"
            and ref.get("function_begin_va") == "0x140b0b1c0"
            for ref in title_table_refs
        )
        and calls_in_range("slot_reset_title_parent_ctor_b0b020", title_ctor_begin, title_ctor_end) == ["0x140b0b1df"],
        "wrapper_captures_table_arg_and_forwards_as_r8": b"\x48\x8b\xda" in wrapper_blob
        and b"\x4c\x8b\xc3" in wrapper_blob
        and calls_in_range("slot_reset_title_parent_base_ctor_b0b0d0", wrapper_begin, wrapper_end) == ["0x140b0b079"],
        "base_ctor_stores_table_pointer_to_owner10": b"\x48\x89\x5e\x10" in base_ctor_blob,
        "base_ctor_initializes_state_fields": b"\x48\x89\x5e\x48" in base_ctor_blob
        and b"\x88\x5e\x50" in base_ctor_blob
        and b"\x48\x89\x46\x58" in base_ctor_blob
        and b"\x48\x89\x5e\x60" in base_ctor_blob
        and b"\x88\x46\x68" in base_ctor_blob
        and b"\x88\x5e\x69" in base_ctor_blob
        and b"\xc7\x86\xac\x00\x00\x00\xff\xff\xff\xff" in base_ctor_blob,
        "title_ctor_sets_final_vtable_and_state0": function_has_rip_lea_any_reg_to(
            data, image_base, sections, title_ctor_begin, 0x142B63BB0, max(0, title_ctor_end - title_ctor_begin)
        )
        and calls_in_range("slot_reset_set_state_helper_b0d960", title_ctor_begin, title_ctor_end) == ["0x140b0b2d8"]
        and b"\x33\xd2" in title_ctor_blob,
        "title_ctor_initializes_owner_128_130": b"\x48\x89\xb7\x28\x01\x00\x00" in title_ctor_blob
        and b"\x48\x8d\x87\x30\x01\x00\x00" in title_ctor_blob
        and b"\x48\x89\x30" in title_ctor_blob,
        "constructor_chain_to_parent_table_dispatch_mapped": b"\x48\x8b\x43\x10" in read_bytes(
            data, image_base, sections, TARGETS["slot_reset_parent_loop_b0bd60"], 0x120
        )
        and b"\x42\xff\x14\xc0" in read_bytes(data, image_base, sections, TARGETS["slot_reset_parent_loop_b0bd60"], 0x120),
        "wrapper_bytes_hex": wrapper_blob.hex(),
        "base_ctor_bytes_hex": base_ctor_blob.hex(),
        "title_ctor_bytes_hex": title_ctor_blob.hex(),
    }


def read_slot_reset_to_menu_job_wait_context(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
    state_table_context: dict[str, Any],
) -> dict[str, Any]:
    helper_va = TARGETS["slot_reset_to_menu_job_wait_helper_b0e530"]
    helper_range = find_pdata_range_for_pc(data, image_base, sections, helper_va)
    helper_begin = int(str(helper_range.get("begin_va")), 16) if helper_range.get("begin_va") else helper_va
    helper_end = int(str(helper_range.get("end_va")), 16) if helper_range.get("end_va") else helper_va + 0x120
    helper_blob = read_bytes(data, image_base, sections, helper_begin, max(0, helper_end - helper_begin))
    bool_wrapper_va = TARGETS["slot_reset_title_begintitle_bool_wrapper_b0c180"]
    input_builder_va = TARGETS["slot_reset_title_accept_input_condition_builder_7acb00"]
    branch_step_va = TARGETS["slot_reset_branch_gate_chain_step_7927d0"]
    owner_builder_va = TARGETS["slot_reset_title_begintitle_owner138_e0_builder_81f9f0"]
    node_update_va = TARGETS["slot_reset_title_accept_input_node_update_7ad1c0"]
    input_manager_state_va = TARGETS["slot_reset_title_accept_input_manager_state_765f20"]
    input_manager_shutdown_va = TARGETS["slot_reset_title_accept_input_manager_shutdown_765fa0"]
    input_manager_init_va = TARGETS["slot_reset_title_accept_input_manager_init_766010"]
    input_manager_queue_setup_va = TARGETS["slot_reset_title_accept_input_manager_queue_setup_7660f0"]
    temp_clone_va = TARGETS["slot_reset_title_accept_input_temp_clone_7ad6c0"]
    temp_callback_va = TARGETS["slot_reset_title_accept_input_temp_callback_7ad810"]
    temp_active_clone_va = TARGETS["slot_reset_title_accept_input_temp_active_clone_7ad990"]
    temp_child_clone_va = TARGETS["slot_reset_title_accept_input_temp_child_clone_7ad9d0"]
    bool_wrapper_range = find_pdata_range_for_pc(data, image_base, sections, bool_wrapper_va)
    input_builder_range = find_pdata_range_for_pc(data, image_base, sections, input_builder_va)
    branch_step_range = find_pdata_range_for_pc(data, image_base, sections, branch_step_va)
    owner_builder_range = find_pdata_range_for_pc(data, image_base, sections, owner_builder_va)
    node_update_range = find_pdata_range_for_pc(data, image_base, sections, node_update_va)
    input_manager_state_range = find_pdata_range_for_pc(data, image_base, sections, input_manager_state_va)
    if input_manager_state_range.get("begin_va") != f"0x{input_manager_state_va:x}":
        input_manager_state_range = {
            "pc_va": f"0x{input_manager_state_va:x}",
            "begin_va": f"0x{input_manager_state_va:x}",
            "end_va": f"0x{input_manager_state_va + 0x11:x}",
            "unwind_va": None,
            "source": "tiny_function_fallback",
        }
    input_manager_shutdown_range = find_pdata_range_for_pc(data, image_base, sections, input_manager_shutdown_va)
    if input_manager_shutdown_range.get("begin_va") == f"0x{input_manager_shutdown_va:x}":
        input_manager_shutdown_range = {
            **input_manager_shutdown_range,
            "end_va": f"0x{input_manager_shutdown_va + 0x65:x}",
            "source": "split_unwind_extended_for_global_clear",
        }
    input_manager_init_range = find_pdata_range_for_pc(data, image_base, sections, input_manager_init_va)
    input_manager_queue_setup_range = find_pdata_range_for_pc(data, image_base, sections, input_manager_queue_setup_va)
    temp_clone_range = find_pdata_range_for_pc(data, image_base, sections, temp_clone_va)
    temp_callback_range = find_pdata_range_for_pc(data, image_base, sections, temp_callback_va)
    temp_active_clone_range = find_pdata_range_for_pc(data, image_base, sections, temp_active_clone_va)
    temp_child_clone_range = find_pdata_range_for_pc(data, image_base, sections, temp_child_clone_va)
    bool_wrapper_begin = int(str(bool_wrapper_range.get("begin_va")), 16) if bool_wrapper_range.get("begin_va") else bool_wrapper_va
    bool_wrapper_end = int(str(bool_wrapper_range.get("end_va")), 16) if bool_wrapper_range.get("end_va") else bool_wrapper_va + 0xAF
    input_builder_begin = int(str(input_builder_range.get("begin_va")), 16) if input_builder_range.get("begin_va") else input_builder_va
    input_builder_end = int(str(input_builder_range.get("end_va")), 16) if input_builder_range.get("end_va") else input_builder_va + 0x190
    branch_step_begin = int(str(branch_step_range.get("begin_va")), 16) if branch_step_range.get("begin_va") else branch_step_va
    branch_step_end = int(str(branch_step_range.get("end_va")), 16) if branch_step_range.get("end_va") else branch_step_va + 0xB9
    owner_builder_begin = int(str(owner_builder_range.get("begin_va")), 16) if owner_builder_range.get("begin_va") else owner_builder_va
    owner_builder_end = int(str(owner_builder_range.get("end_va")), 16) if owner_builder_range.get("end_va") else owner_builder_va + 0xEF
    node_update_begin = int(str(node_update_range.get("begin_va")), 16) if node_update_range.get("begin_va") else node_update_va
    node_update_end = int(str(node_update_range.get("end_va")), 16) if node_update_range.get("end_va") else node_update_va + 0x4FC
    input_manager_state_begin = int(str(input_manager_state_range.get("begin_va")), 16) if input_manager_state_range.get("begin_va") else input_manager_state_va
    input_manager_state_end = int(str(input_manager_state_range.get("end_va")), 16) if input_manager_state_range.get("end_va") else input_manager_state_va + 0x11
    input_manager_shutdown_begin = int(str(input_manager_shutdown_range.get("begin_va")), 16) if input_manager_shutdown_range.get("begin_va") else input_manager_shutdown_va
    input_manager_shutdown_end = int(str(input_manager_shutdown_range.get("end_va")), 16) if input_manager_shutdown_range.get("end_va") else input_manager_shutdown_va + 0x65
    input_manager_init_begin = int(str(input_manager_init_range.get("begin_va")), 16) if input_manager_init_range.get("begin_va") else input_manager_init_va
    input_manager_init_end = int(str(input_manager_init_range.get("end_va")), 16) if input_manager_init_range.get("end_va") else input_manager_init_va + 0xDD
    input_manager_queue_setup_begin = int(str(input_manager_queue_setup_range.get("begin_va")), 16) if input_manager_queue_setup_range.get("begin_va") else input_manager_queue_setup_va
    input_manager_queue_setup_end = int(str(input_manager_queue_setup_range.get("end_va")), 16) if input_manager_queue_setup_range.get("end_va") else input_manager_queue_setup_va + 0x11F
    temp_clone_begin = int(str(temp_clone_range.get("begin_va")), 16) if temp_clone_range.get("begin_va") else temp_clone_va
    temp_clone_end = int(str(temp_clone_range.get("end_va")), 16) if temp_clone_range.get("end_va") else temp_clone_va + 0x3F
    temp_callback_begin = int(str(temp_callback_range.get("begin_va")), 16) if temp_callback_range.get("begin_va") else temp_callback_va
    temp_callback_end = int(str(temp_callback_range.get("end_va")), 16) if temp_callback_range.get("end_va") else temp_callback_va + 0x2A
    temp_active_clone_begin = int(str(temp_active_clone_range.get("begin_va")), 16) if temp_active_clone_range.get("begin_va") else temp_active_clone_va
    temp_active_clone_end = int(str(temp_active_clone_range.get("end_va")), 16) if temp_active_clone_range.get("end_va") else temp_active_clone_va + 0x3F
    temp_child_clone_begin = int(str(temp_child_clone_range.get("begin_va")), 16) if temp_child_clone_range.get("begin_va") else temp_child_clone_va
    temp_child_clone_end = int(str(temp_child_clone_range.get("end_va")), 16) if temp_child_clone_range.get("end_va") else temp_child_clone_va + 0x70
    bool_wrapper_blob = read_bytes(data, image_base, sections, bool_wrapper_begin, max(0, bool_wrapper_end - bool_wrapper_begin))
    input_builder_blob = read_bytes(data, image_base, sections, input_builder_begin, max(0, input_builder_end - input_builder_begin))
    branch_step_blob = read_bytes(data, image_base, sections, branch_step_begin, max(0, branch_step_end - branch_step_begin))
    owner_builder_blob = read_bytes(data, image_base, sections, owner_builder_begin, max(0, owner_builder_end - owner_builder_begin))
    node_update_blob = read_bytes(data, image_base, sections, node_update_begin, max(0, node_update_end - node_update_begin))
    input_manager_state_blob = read_bytes(data, image_base, sections, input_manager_state_begin, max(0, input_manager_state_end - input_manager_state_begin))
    input_manager_shutdown_blob = read_bytes(data, image_base, sections, input_manager_shutdown_begin, max(0, input_manager_shutdown_end - input_manager_shutdown_begin))
    input_manager_init_blob = read_bytes(data, image_base, sections, input_manager_init_begin, max(0, input_manager_init_end - input_manager_init_begin))
    input_manager_queue_setup_blob = read_bytes(data, image_base, sections, input_manager_queue_setup_begin, max(0, input_manager_queue_setup_end - input_manager_queue_setup_begin))
    temp_clone_blob = read_bytes(data, image_base, sections, temp_clone_begin, max(0, temp_clone_end - temp_clone_begin))
    temp_callback_blob = read_bytes(data, image_base, sections, temp_callback_begin, max(0, temp_callback_end - temp_callback_begin))
    temp_active_clone_blob = read_bytes(data, image_base, sections, temp_active_clone_begin, max(0, temp_active_clone_end - temp_active_clone_begin))
    temp_child_clone_blob = read_bytes(data, image_base, sections, temp_child_clone_begin, max(0, temp_child_clone_end - temp_child_clone_begin))

    state_entries = state_table_context.get("entries", [])
    handlers_by_va = {str(entry.get("handler_va")): entry for entry in state_entries if entry.get("handler_va")}

    def calls_in_range(target_name: str, begin: int, end: int) -> list[str]:
        return sorted(
            [
                str(ref.get("source_va"))
                for ref in rel32_refs.get(target_name, [])
                if begin <= int(str(ref.get("source_va")), 16) < end
            ],
            key=lambda value: int(value, 16),
        )

    helper_callers: list[dict[str, Any]] = []
    for ref in sorted(
        rel32_refs.get("slot_reset_to_menu_job_wait_helper_b0e530", []),
        key=lambda item: int(str(item.get("source_va")), 16),
    ):
        source_va = int(str(ref.get("source_va")), 16)
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        label_entry = handlers_by_va.get(str(begin_text)) if begin_text else None
        helper_callers.append(
            {
                "source_va": ref.get("source_va"),
                "function_begin_va": begin_text,
                "function_end_va": function_range.get("end_va"),
                "state_index": label_entry.get("entry_index") if label_entry else None,
                "label_text": label_entry.get("label_text") if label_entry else None,
            }
        )

    caller_target_names = [
        "selector_builder_chain_key_7a91e0",
        "slot_reset_branch_gate_descriptor_wrapper_744a60",
        "task_enqueue_7a7b60",
        "task_enqueue_link_7a7bb0",
        "slot_reset_title_beginlogo_owner_e0_builder_81f180",
        "slot_reset_title_begintitle_owner138_e0_builder_81f9f0",
        "slot_reset_title_begintitle_bool_wrapper_b0c180",
        "slot_reset_title_accept_input_condition_builder_7acb00",
        "slot_reset_branch_gate_chain_compose_78e0e0",
        "slot_reset_branch_gate_chain_step_7927d0",
        "slot_reset_branch_gate_resource_75fbd0",
        "slot_reset_branch_gate_job_7bae20",
        "slot_reset_title_xr_job_7bb010",
        "slot_reset_title_initmenu_gate_builder_7a6c00",
    ]
    caller_contexts: dict[str, dict[str, Any]] = {}
    for caller in helper_callers:
        begin_text = caller.get("function_begin_va")
        end_text = caller.get("function_end_va")
        if not begin_text or not end_text:
            continue
        begin = int(str(begin_text), 16)
        end = int(str(end_text), 16)
        blob = read_bytes(data, image_base, sections, begin, max(0, end - begin))
        calls = {target_name: calls_in_range(target_name, begin, end) for target_name in caller_target_names}
        label = str(caller.get("label_text"))
        caller_contexts[label] = {
            "function_begin_va": begin_text,
            "function_end_va": end_text,
            "handoff_source_va": caller.get("source_va"),
            "calls_by_target": calls,
            "hands_final_enqueue_result_to_helper": bytes.fromhex("488bd0") in blob[max(0, int(str(caller.get("source_va")), 16) - begin - 12) : int(str(caller.get("source_va")), 16) - begin]
            or bytes.fromhex("488bd0") in blob[max(0, int(str(caller.get("source_va")), 16) - begin - 16) : int(str(caller.get("source_va")), 16) - begin],
            "begin_logo_owner_e0_payload_shape": calls.get("slot_reset_title_beginlogo_owner_e0_builder_81f180") == ["0x140b0c43e"]
            and calls.get("task_enqueue_link_7a7bb0") == ["0x140b0c44e"]
            and {"0x140b0c42b", "0x140b0c45b"}.issubset(set(calls.get("task_enqueue_7a7b60", []))),
            "begin_title_accept_payload_shape": calls.get("slot_reset_title_begintitle_owner138_e0_builder_81f9f0") == [
                "0x140b0c70b"
            ]
            and calls.get("slot_reset_title_begintitle_bool_wrapper_b0c180") == ["0x140b0c72a"]
            and calls.get("slot_reset_branch_gate_chain_compose_78e0e0") == ["0x140b0c73a"]
            and calls.get("slot_reset_branch_gate_chain_step_7927d0") == ["0x140b0c74d"]
            and "0x140b0c75b" in calls.get("task_enqueue_7a7b60", []),
            "begin_title_owner_builder_args_owner_e0_138": calls.get(
                "slot_reset_title_begintitle_owner138_e0_builder_81f9f0"
            ) == ["0x140b0c70b"]
            and b"\x4c\x8d\x86\x38\x01\x00\x00" in blob
            and b"\x48\x8d\x96\xe0\x00\x00\x00" in blob,
            "xr_dialog_payload_shape": calls.get("slot_reset_branch_gate_resource_75fbd0") == [
                "0x140b0c9ac",
                "0x140b0c9e0",
            ]
            and calls.get("slot_reset_branch_gate_job_7bae20") == ["0x140b0c9c1"]
            and calls.get("slot_reset_title_xr_job_7bb010") == ["0x140b0c9f5"]
            and {"0x140b0ca06", "0x140b0ca17"}.issubset(set(calls.get("task_enqueue_link_7a7bb0", [])))
            and b"\xba\x4e\x76\x09\x00" in blob
            and b"\xba\x44\x76\x09\x00" in blob,
            "init_menu_two_descriptor_payload_shape": calls.get("selector_builder_chain_key_7a91e0") == [
                "0x140b0d0b6",
                "0x140b0d11a",
            ]
            and calls.get("slot_reset_branch_gate_descriptor_wrapper_744a60") == [
                "0x140b0d0ca",
                "0x140b0d12e",
            ]
            and {"0x140b0d0d8", "0x140b0d13c"}.issubset(set(calls.get("task_enqueue_7a7b60", []))),
            "init_menu_gate_payload_shape": calls.get("slot_reset_title_initmenu_gate_builder_7a6c00") == [
                "0x140b0d185",
                "0x140b0d1d5",
            ]
            and "0x140b0d193" in calls.get("task_enqueue_7a7b60", []),
            "init_menu_link_fold_payload_shape": calls.get("task_enqueue_link_7a7bb0") == [
                "0x140b0d1e8",
                "0x140b0d1fb",
                "0x140b0d20e",
            ]
            and "0x140b0d21c" in calls.get("task_enqueue_7a7b60", []),
            "init_menu_multibranch_payload_shape": calls.get("selector_builder_chain_key_7a91e0") == [
                "0x140b0d0b6",
                "0x140b0d11a",
            ]
            and calls.get("slot_reset_branch_gate_descriptor_wrapper_744a60") == [
                "0x140b0d0ca",
                "0x140b0d12e",
            ]
            and calls.get("slot_reset_title_initmenu_gate_builder_7a6c00") == [
                "0x140b0d185",
                "0x140b0d1d5",
            ]
            and calls.get("task_enqueue_7a7b60") == [
                "0x140b0d0d8",
                "0x140b0d13c",
                "0x140b0d193",
                "0x140b0d21c",
            ]
            and calls.get("task_enqueue_link_7a7bb0") == [
                "0x140b0d1e8",
                "0x140b0d1fb",
                "0x140b0d20e",
            ],
        }

    bool_wrapper_inner_calls = calls_in_range(
        "title_accept_final_wrapper_inner_78c530", bool_wrapper_begin, bool_wrapper_end
    )
    input_builder_calls = {
        "title_accept_primary_chain_builder_7a72b0": calls_in_range(
            "title_accept_primary_chain_builder_7a72b0", input_builder_begin, input_builder_end
        ),
        "slot_reset_title_accept_input_alloc_seed_7a72a0": calls_in_range(
            "slot_reset_title_accept_input_alloc_seed_7a72a0", input_builder_begin, input_builder_end
        ),
        "slot_reset_title_accept_input_node_ctor_7a6f20": calls_in_range(
            "slot_reset_title_accept_input_node_ctor_7a6f20", input_builder_begin, input_builder_end
        ),
    }
    input_builder_callers = sorted(
        [str(ref.get("source_va")) for ref in rel32_refs.get("slot_reset_title_accept_input_condition_builder_7acb00", [])],
        key=lambda value: int(value, 16),
    )
    owner_builder_input_calls = calls_in_range(
        "slot_reset_title_accept_input_condition_builder_7acb00", owner_builder_begin, owner_builder_end
    )
    branch_step_calls = {
        "title_accept_branch_step_builder_792970": calls_in_range(
            "title_accept_branch_step_builder_792970", branch_step_begin, branch_step_end
        ),
        "title_accept_attach_7418d0": calls_in_range("title_accept_attach_7418d0", branch_step_begin, branch_step_end),
    }
    node_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["slot_reset_title_accept_input_node_vtable_aa97e8"], 4
    )
    temp_base_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["slot_reset_title_accept_input_temp_base_vtable_aa9808"], 8
    )
    temp_active_vtable_slots = read_qwords(
        data, image_base, sections, TARGETS["slot_reset_title_accept_input_temp_active_vtable_aa9840"], 12
    )
    temp_callback_calls = calls_in_range(
        "slot_reset_title_accept_input_manager_state_765f20", temp_callback_begin, temp_callback_end
    )
    input_manager_global_refs_near_title_accept = [
        ref
        for ref in scan_rip_relative_refs_to_va(
            data, image_base, sections, TARGETS["slot_reset_title_accept_input_manager_global_43d6b7b0"]
        )
        if temp_callback_begin <= int(str(ref.get("source_va")), 16) < temp_callback_end
    ]
    input_manager_singleton_lifecycle_refs = [
        ref
        for ref in scan_rip_relative_refs_to_va(
            data, image_base, sections, TARGETS["slot_reset_title_accept_input_manager_singleton_43d6b880"]
        )
        if input_manager_shutdown_begin <= int(str(ref.get("source_va")), 16) < input_manager_init_end
    ]
    set_state_calls = calls_in_range("slot_reset_set_state_helper_b0d960", helper_begin, helper_end)
    attach_calls = calls_in_range("slot_reset_branch_gate_submit_attach_7a9460", helper_begin, helper_end)
    return {
        "helper_begin_va": helper_range.get("begin_va"),
        "helper_end_va": helper_range.get("end_va"),
        "helper_callers": helper_callers,
        "caller_contexts": caller_contexts,
        "bool_wrapper_begin_va": bool_wrapper_range.get("begin_va"),
        "bool_wrapper_end_va": bool_wrapper_range.get("end_va"),
        "input_builder_begin_va": input_builder_range.get("begin_va"),
        "input_builder_end_va": input_builder_range.get("end_va"),
        "branch_step_begin_va": branch_step_range.get("begin_va"),
        "branch_step_end_va": branch_step_range.get("end_va"),
        "owner_builder_begin_va": owner_builder_range.get("begin_va"),
        "owner_builder_end_va": owner_builder_range.get("end_va"),
        "node_update_begin_va": node_update_range.get("begin_va"),
        "node_update_end_va": node_update_range.get("end_va"),
        "input_manager_state_begin_va": input_manager_state_range.get("begin_va"),
        "input_manager_state_end_va": input_manager_state_range.get("end_va"),
        "input_manager_shutdown_begin_va": input_manager_shutdown_range.get("begin_va"),
        "input_manager_shutdown_end_va": input_manager_shutdown_range.get("end_va"),
        "input_manager_init_begin_va": input_manager_init_range.get("begin_va"),
        "input_manager_init_end_va": input_manager_init_range.get("end_va"),
        "input_manager_queue_setup_begin_va": input_manager_queue_setup_range.get("begin_va"),
        "input_manager_queue_setup_end_va": input_manager_queue_setup_range.get("end_va"),
        "temp_clone_begin_va": temp_clone_range.get("begin_va"),
        "temp_clone_end_va": temp_clone_range.get("end_va"),
        "temp_callback_begin_va": temp_callback_range.get("begin_va"),
        "temp_callback_end_va": temp_callback_range.get("end_va"),
        "temp_active_clone_begin_va": temp_active_clone_range.get("begin_va"),
        "temp_active_clone_end_va": temp_active_clone_range.get("end_va"),
        "temp_child_clone_begin_va": temp_child_clone_range.get("begin_va"),
        "temp_child_clone_end_va": temp_child_clone_range.get("end_va"),
        "node_vtable_slots": node_vtable_slots,
        "temp_base_vtable_slots": temp_base_vtable_slots,
        "temp_active_vtable_slots": temp_active_vtable_slots,
        "temp_callback_calls": temp_callback_calls,
        "input_manager_global_refs_near_title_accept": input_manager_global_refs_near_title_accept,
        "input_manager_singleton_lifecycle_refs": input_manager_singleton_lifecycle_refs,
        "bool_wrapper_inner_calls": bool_wrapper_inner_calls,
        "input_builder_calls": input_builder_calls,
        "input_builder_callers": input_builder_callers,
        "owner_builder_input_calls": owner_builder_input_calls,
        "branch_step_calls": branch_step_calls,
        "begintitle_bool_wrapper_maps_final_wrapper_inner": bool_wrapper_inner_calls == ["0x140b0c1d9"]
        and b"\x41\x0f\xb6\xf8" in bool_wrapper_blob
        and b"\x44\x0f\xb6\xc7" in bool_wrapper_blob,
        "begintitle_owner_builder_range_mapped": owner_builder_range.get("begin_va") == "0x14081f9f0"
        and owner_builder_range.get("end_va") == "0x14081fae0",
        "begintitle_owner_builder_args_owner_e0_138": caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get(
            "begin_title_owner_builder_args_owner_e0_138"
        ),
        "begintitle_owner_builder_calls_input_condition_at_fa8b": owner_builder_input_calls == ["0x14081fa8b"],
        "begintitle_owner_builder_selector6_active": b"\xc7\x44\x24\x30\x06\x00\x00\x00" in owner_builder_blob
        and b"\xc6\x44\x24\x34\x01" in owner_builder_blob,
        "begintitle_owner_builder_passes_descriptor_to_input_condition": b"\x4d\x8d\x4b\xa8" in owner_builder_blob
        and b"\x4c\x8d\x44\x24\x30" in owner_builder_blob,
        "begintitle_owner_builder_preserves_owner_sources": b"\x4d\x89\x43\xb0" in owner_builder_blob
        and caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get("begin_title_owner_builder_args_owner_e0_138"),
        "begintitle_input_condition_builder_allocates_0x138": b"\xb9\x38\x01\x00\x00" in input_builder_blob,
        "begintitle_input_condition_builder_selector6_active": b"\xc7\x44\x24\x30\x06\x00\x00\x00" in owner_builder_blob
        and b"\xc6\x44\x24\x34\x01" in owner_builder_blob
        and b"\xb9\x38\x01\x00\x00" in input_builder_blob,
        "begintitle_input_condition_builder_owner_sources": b"\x4c\x89\x66\x50" in input_builder_blob
        and b"\x0f\x11\x46\x58" in input_builder_blob
        and b"\x48\x8d\x5e\x70" in input_builder_blob,
        "begintitle_input_builder_range_mapped": input_builder_range.get("begin_va") == "0x1407acb00"
        and input_builder_range.get("end_va") == "0x1407acd32",
        "begintitle_input_builder_captures_all_args": b"\x4d\x8b\xe9" in input_builder_blob
        and b"\x49\x8b\xd8" in input_builder_blob
        and b"\x4c\x8b\xe2" in input_builder_blob
        and b"\x4c\x8b\xf1" in input_builder_blob,
        "begintitle_input_builder_allocates_and_constructs_0x138_node": input_builder_calls.get(
            "slot_reset_title_accept_input_alloc_seed_7a72a0"
        ) == ["0x1407acb4e"]
        and input_builder_calls.get("slot_reset_title_accept_input_node_ctor_7a6f20") == ["0x1407acbda"]
        and b"\xb9\x38\x01\x00\x00" in input_builder_blob,
        "begintitle_input_builder_temp_descriptor_vtables_mapped": function_has_rip_lea_any_reg_to(
            data, image_base, sections, input_builder_begin, TARGETS["slot_reset_title_accept_input_temp_base_vtable_aa9808"], max(0, input_builder_end - input_builder_begin)
        )
        and function_has_rip_lea_any_reg_to(
            data, image_base, sections, input_builder_begin, TARGETS["slot_reset_title_accept_input_temp_active_vtable_aa9840"], max(0, input_builder_end - input_builder_begin)
        )
        and function_has_rip_lea_any_reg_to(
            data, image_base, sections, input_builder_begin, TARGETS["slot_reset_title_accept_input_temp_callback_7ad810"], max(0, input_builder_end - input_builder_begin)
        ),
        "begintitle_input_builder_node_vtable_mapped": function_has_rip_lea_any_reg_to(
            data, image_base, sections, input_builder_begin, TARGETS["slot_reset_title_accept_input_node_vtable_aa97e8"], max(0, input_builder_end - input_builder_begin)
        )
        and b"\x48\x89\x06" in input_builder_blob,
        "begintitle_input_builder_copies_selector_and_sources": b"\x0f\x10\x03" in input_builder_blob
        and b"\x0f\x29\x45\x87" in input_builder_blob
        and b"\x4c\x89\x66\x50" in input_builder_blob
        and b"\x0f\x11\x46\x58" in input_builder_blob
        and b"\x4c\x89\x66\x68" in input_builder_blob,
        "begintitle_input_builder_initializes_condition_subnodes": b"\x48\x8d\x5e\x70" in input_builder_blob
        and b"\x48\x8d\x86\xb0\x00\x00\x00" in input_builder_blob
        and b"\x48\x8d\x9e\xf0\x00\x00\x00" in input_builder_blob
        and b"\x49\x8b\x4d\x38" in input_builder_blob,
        "begintitle_input_builder_returns_output_and_cleans_sources": b"\x49\x89\x36" in input_builder_blob
        and b"\x83\xcf\x04" in input_builder_blob
        and b"\x40\xf6\xc7\x02" in input_builder_blob
        and b"\x40\xf6\xc7\x01" in input_builder_blob,
        "begintitle_input_node_vtable_update_slot_mapped": [row.get("value_va") for row in node_vtable_slots]
        == ["0x140744d90", "0x1407ac850", "0x1407ad1c0", "0x1432f2440"],
        "begintitle_input_node_update_range_mapped": node_update_range.get("begin_va") == "0x1407ad1c0"
        and node_update_range.get("end_va") == "0x1407ad6bc",
        "begintitle_input_node_update_gate_fields_mapped": b"\x48\x83\xb9\x30\x01\x00\x00\x00" in node_update_blob
        and b"\x48\x83\xb9\xa8\x00\x00\x00\x00" in node_update_blob
        and b"\x48\x83\x79\x10\x00" in node_update_blob
        and b"\x48\x83\xb9\xe8\x00\x00\x00\x00" in node_update_blob,
        "begintitle_input_node_update_moves_child_to_130": b"\x48\xc7\x87\x30\x01\x00\x00\x00\x00\x00\x00" in node_update_blob
        and b"\x4c\x89\xbf\x30\x01\x00\x00" in node_update_blob
        and b"\x48\x89\xb7\x30\x01\x00\x00" in node_update_blob,
        "begintitle_input_node_update_status_switch_mapped": b"\x48\x8b\x8f\x28\x01\x00\x00" in node_update_blob
        and b"\xff\x50\x10" in node_update_blob
        and b"\x83\xe8\x01\x74" in node_update_blob
        and b"\x83\xf8\x01" in node_update_blob,
        "begintitle_input_node_update_global_input_bit_mapped": b"\x48\x8b\x35" in node_update_blob
        and b"\x0f\xb7\x03" in node_update_blob
        and b"\x66\x83\xf8\x47" in node_update_blob
        and b"\x42\x80\x8c\x09\x90\x00\x00\x00\x01" in node_update_blob,
        "begintitle_input_node_update_terminal_descriptor_mapped": b"\x48\x8b\x88\xe8\x01\x00\x00" in node_update_blob
        and b"\xe8\x9d\xbb\xff\xff" in node_update_blob
        and b"\x49\x89\x04\x24" in node_update_blob
        and b"\x48\x8d\x05\xda\xb7\x21\x02" in node_update_blob,
        "begintitle_input_node_update_child_submit_mapped": b"\x48\x8b\x97\x30\x01\x00\x00" in node_update_blob
        and b"\x48\x8b\x4f\x50" in node_update_blob
        and b"\xe8\xb0\x69\xf8\xff" in node_update_blob,
        "begintitle_input_node_update_wait_states_mapped": b"\x41\x8d\x50\x03" in node_update_blob
        and b"\x41\x8d\x50\x01" in node_update_blob
        and b"\x48\x8d\x05\xef\xb8\x21\x02" in node_update_blob,
        "begintitle_input_builder_subnode_offsets_match_update": b"\x48\x8d\x5e\x70" in input_builder_blob
        and b"\x48\x8d\x86\xb0\x00\x00\x00" in input_builder_blob
        and b"\x48\x8d\x9e\xf0\x00\x00\x00" in input_builder_blob
        and b"\x48\x83\xb9\xa8\x00\x00\x00\x00" in node_update_blob
        and b"\x48\x83\xb9\xe8\x00\x00\x00\x00" in node_update_blob
        and b"\x48\x83\xbf\x28\x01\x00\x00\x00" in node_update_blob,
        "begintitle_temp_vtable_slots_mapped": [row.get("value_va") for row in temp_active_vtable_slots[:6]]
        == ["0x1407ad6c0", "0x1407ad990", "0x1407ad8c0", "0x1407add80", "0x1407ad840", "0x1407ad970"],
        "begintitle_temp_clone_copies_callback_descriptor": temp_clone_range.get("begin_va") == "0x1407ad6c0"
        and temp_clone_range.get("end_va") == "0x1407ad6ff"
        and b"\x48\x8d\x05\x26\xc1\x2f\x02" in temp_clone_blob
        and b"\x48\x8d\x05\x54\xc1\x2f\x02" in temp_clone_blob
        and b"\x48\x8b\x41\x08" in temp_clone_blob
        and b"\x48\x89\x42\x08" in temp_clone_blob,
        "begintitle_temp_active_clone_copies_callback_descriptor": temp_active_clone_range.get("begin_va") == "0x1407ad990"
        and temp_active_clone_range.get("end_va") == "0x1407ad9cf"
        and b"\x48\x8d\x05\x56\xbe\x2f\x02" in temp_active_clone_blob
        and b"\x48\x8d\x05\x84\xbe\x2f\x02" in temp_active_clone_blob
        and b"\x48\x8b\x41\x08" in temp_active_clone_blob
        and b"\x48\x89\x42\x08" in temp_active_clone_blob,
        "begintitle_temp_child_clone_wraps_source40": temp_child_clone_range.get("begin_va") == "0x1407ad9d0"
        and temp_child_clone_range.get("end_va") == "0x1407ada40"
        and b"\x48\x8d\x05\x5f\xdf\x2e\x02" in temp_child_clone_blob
        and b"\x48\x8d\x05\x75\xbe\x2f\x02" in temp_child_clone_blob
        and b"\x48\x8d\x7a\x08" in temp_child_clone_blob
        and b"\x48\x8b\x49\x40" in temp_child_clone_blob
        and b"\x48\x89\x47\x38" in temp_child_clone_blob,
        "begintitle_temp_callback_inverts_input_manager_state": temp_callback_range.get("begin_va") == "0x1407ad810"
        and temp_callback_range.get("end_va") == "0x1407ad83a"
        and temp_callback_calls == ["0x1407ad827"]
        and b"\x48\x8b\x0d\x95\xdf\x5b\x03" in temp_callback_blob
        and b"\x84\xc0" in temp_callback_blob
        and b"\x0f\x94\xc1" in temp_callback_blob,
        "begintitle_input_manager_state_active18_mapped": input_manager_state_range.get("begin_va") == "0x140765f20"
        and input_manager_state_range.get("end_va") == "0x140765f31"
        and b"\x80\xb9\x98\x08\x00\x00\x00" in input_manager_state_blob
        and b"\x0f\xb6\x41\x18" in input_manager_state_blob,
        "begintitle_input_manager_global_callback_ref_mapped": [
            ref.get("source_va") for ref in input_manager_global_refs_near_title_accept
        ] == ["0x1407ad814"],
        "begintitle_input_manager_singleton_lifecycle_refs_mapped": [
            ref.get("source_va") for ref in input_manager_singleton_lifecycle_refs
        ] == ["0x140765fbb", "0x140765ff4", "0x14076602a", "0x140766068", "0x140766092", "0x1407660c5"],
        "begintitle_input_manager_shutdown_clears_singleton": input_manager_shutdown_range.get("begin_va") == "0x140765fa0"
        and input_manager_shutdown_range.get("end_va") == "0x140766005"
        and b"\xc6\x41\x1a\x00" in input_manager_shutdown_blob
        and b"\x48\x8b\x89\x80\x00\x00\x00" in input_manager_shutdown_blob
        and b"\x48\x8b\x3d\xbe\x58\x60\x03" in input_manager_shutdown_blob
        and b"\x48\xc7\x05\x81\x58\x60\x03\x00\x00\x00\x00" in input_manager_shutdown_blob,
        "begintitle_input_manager_init_allocates_singleton": input_manager_init_range.get("begin_va") == "0x140766010"
        and input_manager_init_range.get("end_va") == "0x1407660ed"
        and b"\x48\x83\x3d\x4e\x58\x60\x03\x00" in input_manager_init_blob
        and b"\xb9\x20\x84\x00\x00" in input_manager_init_blob
        and b"\xba\x10\x00\x00\x00" in input_manager_init_blob
        and b"\xe8\x49\x46\x00\x00" in input_manager_init_blob
        and b"\x48\x89\x05\x11\x58\x60\x03" in input_manager_init_blob,
        "begintitle_input_manager_init_updates_subsystems": b"\x48\x8b\x8b\x80\x00\x00\x00" in input_manager_init_blob
        and b"\xe8\xc0\x77\x08\x00" in input_manager_init_blob
        and b"\x48\x8b\x4b\x08" in input_manager_init_blob
        and b"\xe8\xd7\x1b\x00\x00" in input_manager_init_blob
        and b"\xc6\x43\x1a\x01" in input_manager_init_blob,
        "begintitle_input_manager_init_captures_frame_counter": b"\xe8\xe2\x2e\xf1\xff" in input_manager_init_blob
        and b"\xe8\x9f\x2e\xf1\xff" in input_manager_init_blob
        and b"\x89\x87\x34\x65\x00\x00" in input_manager_init_blob,
        "begintitle_input_manager_queue_setup_allocates_child80": input_manager_queue_setup_range.get("begin_va") == "0x1407660f0"
        and input_manager_queue_setup_range.get("end_va") == "0x14076620f"
        and b"\x48\x8d\x91\xb8\x06\x00\x00" in input_manager_queue_setup_blob
        and b"\x48\x8d\x8b\x88\x00\x00\x00" in input_manager_queue_setup_blob
        and b"\xb9\x20\x03\x00\x00" in input_manager_queue_setup_blob
        and b"\x48\x89\x83\x80\x00\x00\x00" in input_manager_queue_setup_blob
        and b"\x48\x8b\x93\x90\x08\x00\x00" in input_manager_queue_setup_blob,
        "begintitle_branch_step_wrapper_maps_builder_and_attach": branch_step_calls.get(
            "title_accept_branch_step_builder_792970"
        ) == ["0x140792831"]
        and branch_step_calls.get("title_accept_attach_7418d0") == ["0x14079283d"]
        and b"\x49\x8b\xf0" in branch_step_blob
        and b"\x48\xc7\x06\x00\x00\x00\x00" in branch_step_blob,
        "begintitle_accept_condition_chain_mapped": caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get(
            "begin_title_accept_payload_shape"
        )
        and caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get("begin_title_owner_builder_args_owner_e0_138")
        and bool_wrapper_inner_calls == ["0x140b0c1d9"]
        and owner_builder_input_calls == ["0x14081fa8b"]
        and b"\xc7\x44\x24\x30\x06\x00\x00\x00" in owner_builder_blob
        and b"\xc6\x44\x24\x34\x01" in owner_builder_blob
        and branch_step_calls.get("title_accept_branch_step_builder_792970") == ["0x140792831"],
        "attach_calls": attach_calls,
        "set_state_calls": set_state_calls,
        "sets_global_job_active_flag": b"\xc6\x80\xb0\x06\x00\x00\x01" in helper_blob,
        "attaches_payload_to_owner130": b"\x48\x8d\x8e\x30\x01\x00\x00" in helper_blob
        and attach_calls == ["0x140b0e5be"],
        "sets_state10_menujobwait": b"\xba\x0a\x00\x00\x00" in helper_blob
        and set_state_calls == ["0x140b0e604"],
        "releases_payload_after_transition": b"\x48\x8b\x1f" in helper_blob
        and b"\x48\xc7\x07\x00\x00\x00\x00" in helper_blob,
        "callers_are_title_bootstrap_states": [caller.get("label_text") for caller in helper_callers]
        == [
            "TitleStep::STEP_BeginLogo",
            "TitleStep::STEP_BeginTitle",
            "TitleStep::STEP_BeginXR117Dialog",
            "TitleStep::STEP_InitMenu",
        ],
        "candidate_title_to_menujobwait_handoff_mapped": b"\x48\x8d\x8e\x30\x01\x00\x00" in helper_blob
        and b"\xba\x0a\x00\x00\x00" in helper_blob
        and [caller.get("state_index") for caller in helper_callers] == [2, 3, 1, 0],
        "begin_logo_payload_context_mapped": caller_contexts.get("TitleStep::STEP_BeginLogo", {}).get(
            "begin_logo_owner_e0_payload_shape"
        ),
        "begin_title_accept_payload_context_mapped": caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get(
            "begin_title_accept_payload_shape"
        ),
        "xr_dialog_payload_context_mapped": caller_contexts.get("TitleStep::STEP_BeginXR117Dialog", {}).get(
            "xr_dialog_payload_shape"
        ),
        "init_menu_two_descriptor_payload_context_mapped": caller_contexts.get("TitleStep::STEP_InitMenu", {}).get(
            "init_menu_two_descriptor_payload_shape"
        ),
        "init_menu_gate_payload_context_mapped": caller_contexts.get("TitleStep::STEP_InitMenu", {}).get(
            "init_menu_gate_payload_shape"
        ),
        "init_menu_link_fold_payload_context_mapped": caller_contexts.get("TitleStep::STEP_InitMenu", {}).get(
            "init_menu_link_fold_payload_shape"
        ),
        "init_menu_payload_context_mapped": caller_contexts.get("TitleStep::STEP_InitMenu", {}).get(
            "init_menu_multibranch_payload_shape"
        ),
        "all_handoff_callers_payload_context_mapped": bool(
            caller_contexts.get("TitleStep::STEP_BeginLogo", {}).get("begin_logo_owner_e0_payload_shape")
            and caller_contexts.get("TitleStep::STEP_BeginTitle", {}).get("begin_title_accept_payload_shape")
            and caller_contexts.get("TitleStep::STEP_BeginXR117Dialog", {}).get("xr_dialog_payload_shape")
            and caller_contexts.get("TitleStep::STEP_InitMenu", {}).get("init_menu_multibranch_payload_shape")
        ),
        "helper_bytes_hex": helper_blob.hex(),
    }


def read_entry_selector_window(data: bytes, image_base: int, sections: list[dict[str, int | str]]) -> dict[str, Any]:
    start_va = TARGETS["entry_selector_824b50"]
    size = 0xC0
    blob = read_bytes(data, image_base, sections, start_va, size)
    return {
        "start_va": f"0x{start_va:x}",
        "start_rva": f"0x{start_va - image_base:x}",
        "size": size,
        "bytes_hex": blob.hex(),
        "writes_r8d_to_descriptor_plus4": b"\x48\x8b\x01\x44\x89\x40\x04" in blob,
        "branches_on_selector_minus_one_le_one": b"\x41\x8d\x40\xff" in blob and b"\x83\xf8\x01\x76" in blob,
        "has_alloc_high_path": any(ref["source_va"] == "0x140824b88" for ref in scan_rel32_calls(data, image_base, sections)["task_alloc_selector_high_7a7200"]),
        "has_alloc_low_path": any(ref["source_va"] == "0x140824ba3" for ref in scan_rel32_calls(data, image_base, sections)["task_alloc_selector_low_7a7250"]),
    }


def read_task_local_wrapper_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    contexts: list[dict[str, Any]] = []
    enqueue_targets = {
        TARGETS["task_enqueue_7a7b60"]: "task_enqueue_7a7b60",
        TARGETS["task_enqueue_link_7a7bb0"]: "task_enqueue_link_7a7bb0",
    }
    for ref in rel32_refs.get("task_local_wrapper_7449e0", []):
        source_va = int(str(ref["source_va"]), 16)
        if not (0x140820000 <= source_va <= 0x140830000):
            continue
        function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        window_start = source_va - 0x60
        if begin_text is not None:
            window_start = max(window_start, int(begin_text, 16))
        window_end = source_va + 0x50
        if end_text is not None:
            window_end = min(window_end, int(end_text, 16))
        blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
        rip_leas: list[dict[str, str]] = []
        enqueue_calls: list[dict[str, str]] = []
        for index in range(max(0, len(blob) - 7)):
            if blob[index : index + 3] in (b"\x48\x8d\x05", b"\x4c\x8d\x05"):
                disp = struct.unpack_from("<i", blob, index + 3)[0]
                target = window_start + index + 7 + disp
                if 0x142AC7000 <= target <= 0x142AC7A00:
                    rip_leas.append({"source_va": f"0x{window_start + index:x}", "target_va": f"0x{target:x}"})
        for index in range(max(0, len(blob) - 5)):
            if blob[index] not in (0xE8, 0xE9):
                continue
            rel = struct.unpack_from("<i", blob, index + 1)[0]
            call_source = window_start + index
            target = call_source + 5 + rel
            target_name = enqueue_targets.get(target)
            if target_name is not None:
                enqueue_calls.append({"source_va": f"0x{call_source:x}", "target_name": target_name, "target_va": f"0x{target:x}"})
        contexts.append(
            {
                "source_va": ref["source_va"],
                "function_begin_va": begin_text,
                "function_end_va": end_text,
                "context_start_va": f"0x{window_start:x}",
                "context_bytes_hex": blob.hex(),
                "nearby_rip_lea_vtables": rip_leas,
                "nearby_enqueue_calls": enqueue_calls,
            }
        )
    return contexts


def summarize_task_local_wrapper_sequences(contexts: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    grouped: dict[str, dict[str, Any]] = {}
    for context in contexts:
        function_begin = str(context.get("function_begin_va"))
        entry = grouped.setdefault(
            function_begin,
            {
                "function_begin_va": function_begin,
                "function_end_va": context.get("function_end_va"),
                "wrapper_sources": [],
                "descriptor_vtables": [],
                "enqueue_calls": [],
            },
        )
        entry["wrapper_sources"].append(context.get("source_va"))
        for lea in context.get("nearby_rip_lea_vtables", []):
            target = lea.get("target_va")
            if target not in entry["descriptor_vtables"]:
                entry["descriptor_vtables"].append(target)
        for call in context.get("nearby_enqueue_calls", []):
            if call not in entry["enqueue_calls"]:
                entry["enqueue_calls"].append(call)
    return grouped


def read_builder_constructor_call_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in BUILDER_CONSTRUCTOR_TARGETS:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x30
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x10
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "kind": ref["kind"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "unwind_va": function_range.get("unwind_va"),
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_wrapper_thunk_call_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in WRAPPER_THUNK_TARGETS:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x40
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "kind": ref["kind"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "unwind_va": function_range.get("unwind_va"),
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_outer_thunk_call_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in OUTER_THUNK_TARGETS:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x40
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "kind": ref["kind"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "unwind_va": function_range.get("unwind_va"),
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_entry_thunk_call_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    rel32_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in ENTRY_THUNK_TARGETS:
        target_contexts: list[dict[str, Any]] = []
        for ref in rel32_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            function_range = find_pdata_range_for_pc(data, image_base, sections, source_va)
            begin_text = function_range.get("begin_va")
            end_text = function_range.get("end_va")
            window_start = source_va - 0x40
            if begin_text is not None:
                window_start = max(window_start, int(begin_text, 16))
            window_end = source_va + 0x18
            if end_text is not None:
                window_end = min(window_end, int(end_text, 16))
            blob = read_bytes(data, image_base, sections, window_start, max(0, window_end - window_start))
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "kind": ref["kind"],
                    "target_va": ref["target_va"],
                    "function_begin_va": begin_text,
                    "function_end_va": end_text,
                    "unwind_va": function_range.get("unwind_va"),
                    "context_start_va": f"0x{window_start:x}",
                    "context_bytes_hex": blob.hex(),
                    "moves_rcx_plus8_to_r8": b"\x4c\x8d\x41\x08" in blob,
                    "moves_incoming_rdx_to_rbx": b"\x48\x8b\xda" in blob,
                    "moves_rbx_to_rcx_before_call": source_va - window_start >= 3
                    and blob[source_va - window_start - 3 : source_va - window_start] == b"\x48\x8b\xcb",
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_entry_helper_vtable_contexts(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    absolute_refs: dict[str, list[dict[str, Any]]],
) -> dict[str, list[dict[str, Any]]]:
    contexts: dict[str, list[dict[str, Any]]] = {}
    for target_name in ENTRY_HELPER_TARGETS:
        target_contexts: list[dict[str, Any]] = []
        for ref in absolute_refs.get(target_name, []):
            source_va = int(str(ref["source_va"]), 16)
            vtable_base = source_va - 0x10
            plus8_accessor_va = read_qword_value(data, image_base, sections, source_va - 0x20)
            ctor_va = read_qword_value(data, image_base, sections, vtable_base)
            copy_or_clone_va = read_qword_value(data, image_base, sections, vtable_base + 8)
            helper_va = read_qword_value(data, image_base, sections, vtable_base + 0x10)
            plus8_accessor_bytes = read_bytes(data, image_base, sections, plus8_accessor_va or 0, 5)
            context_rows = read_qwords(data, image_base, sections, source_va - 0x30, 13)
            target_contexts.append(
                {
                    "source_va": ref["source_va"],
                    "source_rva": ref["source_rva"],
                    "section": ref["section"],
                    "target_va": ref["target_va"],
                    "vtable_base_va": f"0x{vtable_base:x}",
                    "task_plus8_accessor_slot_va": f"0x{source_va - 0x20:x}",
                    "task_plus8_accessor_va": f"0x{plus8_accessor_va:x}" if plus8_accessor_va is not None else None,
                    "constructor_slot_va": f"0x{vtable_base:x}",
                    "constructor_va": f"0x{ctor_va:x}" if ctor_va is not None else None,
                    "copy_or_clone_slot_va": f"0x{vtable_base + 8:x}",
                    "copy_or_clone_va": f"0x{copy_or_clone_va:x}" if copy_or_clone_va is not None else None,
                    "helper_slot_va": f"0x{vtable_base + 0x10:x}",
                    "helper_va": f"0x{helper_va:x}" if helper_va is not None else None,
                    "helper_matches_target": helper_va == ENTRY_HELPER_TARGETS[target_name],
                    "task_plus8_accessor_returns_rcx_plus8": plus8_accessor_bytes == b"\x48\x8d\x41\x08\xc3",
                    "constructor_stores_vtable_base": function_has_rip_lea_to(data, image_base, sections, ctor_va, vtable_base),
                    "copy_or_clone_stores_vtable_base": function_has_rip_lea_to(data, image_base, sections, copy_or_clone_va, vtable_base),
                    "vtable_context_start_va": f"0x{source_va - 0x30:x}",
                    "vtable_context_qwords": context_rows,
                }
            )
        contexts[target_name] = target_contexts
    return contexts


def read_builder_argument_flows(
    data: bytes,
    image_base: int,
    sections: list[dict[str, int | str]],
    function_ranges: dict[str, dict[str, str | None]],
) -> dict[str, dict[str, Any]]:
    flows: dict[str, dict[str, Any]] = {}
    for name, call_va in BUILDER_CALLER_SITES.items():
        function_range = function_ranges.get(name, {})
        begin_text = function_range.get("begin_va")
        end_text = function_range.get("end_va")
        if begin_text is None or end_text is None:
            flows[name] = {"call_va": f"0x{call_va:x}", "rcx_source": None, "rdx_source": None}
            continue
        begin_va = int(begin_text, 16)
        end_va = int(end_text, 16)
        blob = read_bytes(data, image_base, sections, begin_va, end_va - begin_va)
        call_offset = call_va - begin_va
        has_incoming_rdx_to_rbx = b"\x48\x8b\xda" in blob[:call_offset]
        has_rbx_to_rcx_before_call = call_offset >= 3 and blob[call_offset - 3 : call_offset] == b"\x48\x8b\xcb"
        has_local_descriptor_to_rdx_before_call = call_offset >= 7 and blob[call_offset - 7 : call_offset - 3] == b"\x49\x8d\x53\xb0"
        flows[name] = {
            "function_begin_va": begin_text,
            "function_end_va": end_text,
            "call_va": f"0x{call_va:x}",
            "call_target_va": f"0x{TARGETS['pump_owner_builder_828fd0']:x}",
            "rcx_source": "incoming_rdx_via_rbx" if has_incoming_rdx_to_rbx and has_rbx_to_rcx_before_call else None,
            "rdx_source": "stack_descriptor_r11_minus_0x50" if has_local_descriptor_to_rdx_before_call else None,
            "xmm2_source": "rip_float_300s" if name == "positive_delay_builder_caller" else "zero",
            "has_incoming_rdx_to_rbx": has_incoming_rdx_to_rbx,
            "has_rbx_to_rcx_before_call": has_rbx_to_rcx_before_call,
            "has_local_descriptor_to_rdx_before_call": has_local_descriptor_to_rdx_before_call,
        }
    return flows


def read_builder_callsite_windows(
    data: bytes, image_base: int, sections: list[dict[str, int | str]]
) -> dict[str, dict[str, Any]]:
    windows: dict[str, dict[str, Any]] = {}
    for name, (start_va, size) in BUILDER_CALLSITE_WINDOWS.items():
        blob = read_bytes(data, image_base, sections, start_va, size)
        wrapper_target = None
        xmm2_mode = None
        xmm2_value = None
        for index in range(max(0, len(blob) - 7)):
            if blob[index : index + 3] == b"\x48\x8d\x05":
                disp = struct.unpack_from("<i", blob, index + 3)[0]
                target = start_va + index + 7 + disp
                # The callsites write the chosen menu wrapper callback right after this LEA.
                if index + 11 <= len(blob) and blob[index + 7 : index + 11] == b"\x49\x89\x43\xb8":
                    wrapper_target = target
            if blob[index : index + 4] == b"\xf3\x0f\x10\x15":
                disp = struct.unpack_from("<i", blob, index + 4)[0]
                target = start_va + index + 8 + disp
                file_offset = va_to_file_offset(image_base, sections, target)
                if file_offset is not None and file_offset + 4 <= len(data):
                    xmm2_mode = "rip_float"
                    xmm2_value = struct.unpack_from("<f", data, file_offset)[0]
            if blob[index : index + 3] == b"\x0f\x57\xd2":
                xmm2_mode = "zero"
                xmm2_value = 0.0
        windows[name] = {
            "start_va": f"0x{start_va:x}",
            "start_rva": f"0x{start_va - image_base:x}",
            "size": size,
            "bytes_hex": blob.hex(),
            "wrapper_target_va": f"0x{wrapper_target:x}" if wrapper_target is not None else None,
            "xmm2_mode": xmm2_mode,
            "xmm2_value": xmm2_value,
            "calls_builder": b"\xe8" in blob and any(
                start_va + idx + 5 + struct.unpack_from("<i", blob, idx + 1)[0] == TARGETS["pump_owner_builder_828fd0"]
                for idx in range(max(0, len(blob) - 4))
                if blob[idx] == 0xE8 and idx + 5 <= len(blob)
            ),
        }
    return windows


def scan_rip_relative_vtable_refs(
    data: bytes, image_base: int, sections: list[dict[str, int | str]]
) -> list[dict[str, Any]]:
    text = next(section for section in sections if section["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    text_data = data[raw_ptr : raw_ptr + raw_size]
    low = min(VTABLE_GROUP_BASES.values()) - 0x20
    high = max(VTABLE_GROUP_BASES.values()) + 0x40
    refs: list[dict[str, Any]] = []
    for index in range(0, max(0, len(text_data) - 7)):
        if text_data[index] not in (0x48, 0x4C):
            continue
        disp = struct.unpack_from("<i", text_data, index + 3)[0]
        target = text_va + index + 7 + disp
        if low <= target < high:
            group = next((name for name, base in VTABLE_GROUP_BASES.items() if base == target), None)
            refs.append(
                {
                    "source_va": f"0x{text_va + index:x}",
                    "source_rva": f"0x{text_va + index - image_base:x}",
                    "target_va": f"0x{target:x}",
                    "target_rva": f"0x{target - image_base:x}",
                    "target_group": group,
                    "bytes_hex": text_data[index : index + 7].hex(),
                }
            )
    return refs


def summarize_autoload_native_transition_candidates(
    selector6_owner_compose_parent_contexts: list[dict[str, Any]],
    selector6_owner_variant_caller_contexts: dict[str, list[dict[str, Any]]],
    selector_owner_lifecycle_context: dict[str, Any],
    slot_reset_state_table_init_context: dict[str, Any],
    slot_reset_play_game_context: dict[str, Any],
    slot_reset_play_game_submit_context: dict[str, Any],
    set_save_slot_callsite_contexts: list[dict[str, Any]],
) -> dict[str, Any]:
    """Condense static evidence for native title/menu progression candidates.

    This deliberately does not claim runtime success. It highlights exact
    scheduler/input/menu-task relationships that could replace host key/focus
    nudges once a safe event-driven runtime probe is redesigned.
    """

    input_parent_contexts = [
        context
        for context in selector6_owner_compose_parent_contexts
        if context.get("passes_common_args_via_input_key")
        or context.get("input_key_calls")
        or any(
            target.get("target_name") == "owner_parent_input_tag_7108"
            for target in context.get("lea_targets", [])
        )
    ]
    input_variant_callers = selector6_owner_variant_caller_contexts.get(
        "selector6_owner_variant_input_82a970", []
    )
    set_slot_menu_wrappers = [
        context
        for context in set_save_slot_callsite_contexts
        if context.get("source_va") == "0x14082c379" and context.get("menu_region")
    ]
    title_state_entries = [
        entry
        for entry in slot_reset_state_table_init_context.get("entries", [])
        if entry.get("label_text")
        in {
            "TitleStep::STEP_MenuJobWait",
            "TitleStep::STEP_PlayGame",
            "TitleStep::STEP_Finish",
        }
    ]
    factory_context = selector_owner_lifecycle_context.get("factory", {})

    return {
        "status": "static_candidate_only_no_runtime_claim",
        "selector_input_parent_contexts": input_parent_contexts,
        "selector_input_variant_callers": input_variant_callers,
        "selector_owner_factory": {
            key: factory_context.get(key)
            for key in [
                "function_begin_va",
                "function_end_va",
                "caller_chain",
                "calls_main_preflight_ctor_wrapper",
                "calls_sibling_ctor_wrapper",
                "main_call_uses_r14_plus_0x18_and_incoming_r8",
                "sibling_call_uses_r14_plus_0x18_and_incoming_r8",
                "enqueue_calls",
                "enqueue_link_calls",
            ]
        },
        "title_state_entries": title_state_entries,
        "set_slot_menu_wrappers": set_slot_menu_wrappers,
        "play_game_submit": {
            key: slot_reset_play_game_submit_context.get(key)
            for key in [
                "function_begin_va",
                "function_end_va",
                "returns_early_when_slot_minus_one",
                "calls_selected_value_validate_then_load_pair",
                "stores_load_pair_to_owner_job_100_104",
                "appends_payload_vector_to_owner_job_b35f0",
            ]
        },
        "play_game_handler": {
            key: slot_reset_play_game_context.get(key)
            for key in [
                "function_begin_va",
                "function_end_va",
                "submits_owner_bc_and_owner_2e8_job",
                "gets_save_slot_and_stores_nonnegative_to_global_1200",
                "tail_function_begin_va",
                "tail_function_end_va",
            ]
        },
        "interpretation": (
            "The selector input variant at 0x14082a970 builds an owner-parent "
            "input descriptor (owner_parent_input_tag_7108 / vtable_76f0) and "
            "routes it into the same parent composer as local menu wrappers. "
            "Its callback target 0x14082c240 is the menu set-slot wrapper that "
            "calls set_save_slot. The TitleStep PlayGame path then validates a "
            "nonnegative slot and stores the load pair before appending the owner "
            "job payload. This is a candidate native/menu-task route for clearing "
            "title-slot selection without host focus/key nudges, but it still "
            "needs runtime validation behind the fail-closed runtime contract."
        ),
    }


def main() -> int:
    exe = Path(os.environ.get("ER_EXE_PATH", str(DEFAULT_EXE)))
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(".auto/last-measure/static-re-evidence.json")
    data = exe.read_bytes()
    exe_md5 = hashlib.md5(data).hexdigest()
    image_base, sections = parse_pe(data)
    rel32_refs = scan_rel32_calls(data, image_base, sections)
    absolute_refs = scan_absolute_qword_refs(data, image_base, sections)
    field_windows = scan_field_windows(data, image_base, sections)
    jump_tables = read_jump_tables(data, image_base, sections)
    switch_target_windows = read_switch_target_windows(data, image_base, sections)
    task_object_windows = read_task_object_windows(data, image_base, sections)
    menu_load_pump_handoff_context = read_menu_load_pump_handoff_context(data, image_base, sections, rel32_refs)
    builder_callsite_windows = read_builder_callsite_windows(data, image_base, sections)
    function_ranges = read_function_ranges(data, image_base, sections)
    builder_argument_flows = read_builder_argument_flows(data, image_base, sections, function_ranges)
    builder_constructor_call_contexts = read_builder_constructor_call_contexts(data, image_base, sections, rel32_refs)
    wrapper_thunk_call_contexts = read_wrapper_thunk_call_contexts(data, image_base, sections, rel32_refs)
    outer_thunk_call_contexts = read_outer_thunk_call_contexts(data, image_base, sections, rel32_refs)
    entry_thunk_call_contexts = read_entry_thunk_call_contexts(data, image_base, sections, rel32_refs)
    entry_helper_vtable_contexts = read_entry_helper_vtable_contexts(data, image_base, sections, absolute_refs)
    entry_vtable_rip_refs = scan_entry_vtable_rip_refs(data, image_base, sections)
    selector_entry_vtable_rip_refs = scan_selector_entry_vtable_rip_refs(data, image_base, sections)
    entry_family_descriptor_rip_refs = scan_entry_family_descriptor_rip_refs(data, image_base, sections)
    entry_family_builder_call_contexts = read_entry_family_builder_call_contexts(data, image_base, sections, rel32_refs)
    entry_selector_contexts = read_entry_selector_contexts(data, image_base, sections, rel32_refs)
    entry_selector_parent_contexts = read_entry_selector_parent_contexts(data, image_base, sections, rel32_refs)
    selector_parent_thunk_contexts = read_selector_chain_contexts(data, image_base, sections, rel32_refs, ENTRY_SELECTOR_PARENT_TARGET_NAMES)
    selector_outer_thunk_contexts = read_selector_chain_contexts(data, image_base, sections, rel32_refs, ENTRY_SELECTOR_OUTER_TARGET_NAMES)
    entry_level_helper_contexts = read_entry_level_helper_contexts(data, image_base, sections, rel32_refs)
    selector_entry_helper_vtable_contexts = read_selector_entry_helper_vtable_contexts(data, image_base, sections, absolute_refs)
    selector6_builder_context = read_selector6_builder_context(data, image_base, sections, rel32_refs)
    selector6_builder_direct_caller_context = read_selector6_builder_direct_caller_context(
        data, image_base, sections, rel32_refs
    )
    selector6_builder_entry_owner_context = read_selector6_builder_entry_owner_context(
        data, image_base, sections, rel32_refs
    )
    selector6_owner_compose_parent_contexts = read_selector6_owner_compose_parent_contexts(
        data, image_base, sections, rel32_refs
    )
    selector6_owner_variant_caller_contexts = read_selector6_owner_variant_caller_contexts(
        data, image_base, sections, rel32_refs
    )
    continue_selector_dispatch_comparison = read_continue_selector_dispatch_comparison(
        data, image_base, sections, rel32_refs, absolute_refs
    )
    selector_owner_lifecycle_context = read_selector_owner_lifecycle_context(data, image_base, sections, rel32_refs)
    selector_owner_factory_entry_context = read_selector_owner_factory_entry_context(
        data, image_base, sections, rel32_refs, absolute_refs
    )
    selector_submit_context = read_selector_submit_context(data, image_base, sections, rel32_refs)
    selector_final_enqueue_context = read_selector_final_enqueue_context(data, image_base, sections, rel32_refs)
    selector_pair_builder_context = read_selector_pair_builder_context(data, image_base, sections, rel32_refs)
    set_save_slot_callsite_contexts = read_set_save_slot_callsite_contexts(data, image_base, sections, rel32_refs)
    game_man_b5e_context = read_game_man_b5e_context(data, image_base, sections, rel32_refs)
    slot_reset_dispatch_context = read_slot_reset_dispatch_context(data, image_base, sections, rel32_refs, absolute_refs)
    slot_reset_title_parent_ctor_context = read_slot_reset_title_parent_ctor_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_state_table_init_context = read_slot_reset_state_table_init_context(data, image_base, sections)
    slot_reset_to_menu_job_wait_context = read_slot_reset_to_menu_job_wait_context(
        data, image_base, sections, rel32_refs, slot_reset_state_table_init_context
    )
    slot_reset_selected_value_field_access_context = read_slot_reset_selected_value_field_access_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_selected_value_caller_context = read_slot_reset_selected_value_caller_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_selected_value_context = read_slot_reset_selected_value_context(data, image_base, sections, rel32_refs)
    slot_reset_play_game_submit_context = read_slot_reset_play_game_submit_context(data, image_base, sections, rel32_refs)
    slot_reset_play_game_context = read_slot_reset_play_game_context(data, image_base, sections, rel32_refs)
    slot_reset_title_queue_state_table_context = read_slot_reset_title_queue_state_table_context(data, image_base, sections)
    slot_reset_title_queue_producer_context = read_slot_reset_title_queue_producer_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_menu_job_wait_context = read_slot_reset_menu_job_wait_context(data, image_base, sections, rel32_refs)
    slot_reset_global_toggle_context = read_slot_reset_global_toggle_context(data, image_base, sections, rel32_refs)
    slot_reset_timed_queue_context = read_slot_reset_timed_queue_context(data, image_base, sections, rel32_refs)
    title_accept_payload_context = read_title_accept_payload_context(data, image_base, sections, rel32_refs)
    slot_reset_end_flow_branch_gate_table_init_context = read_slot_reset_end_flow_branch_gate_table_init_context(
        data, image_base, sections
    )
    finish_gate_synchronization_context = read_finish_gate_synchronization_context(data, image_base, sections)
    slot_reset_end_flow_branch_gate_state_handlers_context = read_slot_reset_end_flow_branch_gate_state_handlers_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_end_flow_branch_gate_context = read_slot_reset_end_flow_branch_gate_context(
        data, image_base, sections, rel32_refs
    )
    slot_reset_end_flow_wait_context = read_slot_reset_end_flow_wait_context(data, image_base, sections, rel32_refs)
    slot_reset_end_flow_tail_branch_context = read_slot_reset_end_flow_tail_branch_context(
        data, image_base, sections, rel32_refs
    )
    entry_selector_window = read_entry_selector_window(data, image_base, sections)
    task_local_wrapper_contexts = read_task_local_wrapper_contexts(data, image_base, sections, rel32_refs)
    task_local_wrapper_sequences = summarize_task_local_wrapper_sequences(task_local_wrapper_contexts)
    rip_relative_vtable_refs = scan_rip_relative_vtable_refs(data, image_base, sections)
    ghidra_reconciliation = build_ghidra_reconciliation(Path.cwd(), image_base, rel32_refs, absolute_refs, exe_md5)

    # The absolute qword reference to 0x14082a0f0 is a vtable/update-table entry;
    # keep nearby entries so future code can identify the owning menu task object.
    pump_abs_refs = absolute_refs["menu_task_update_wrapper"]
    vtable_context: list[dict[str, str]] = []
    if pump_abs_refs:
        first_ref_va = int(pump_abs_refs[0]["source_va"], 16)
        vtable_context = read_qwords(data, image_base, sections, first_ref_va - 0x20, 12)

    request_save_refs = ref_source_set(rel32_refs, "request_save_67a520")
    save_profile_refs = ref_source_set(rel32_refs, "save_request_profile_67a420")
    gate_refs = ref_source_set(rel32_refs, "request_save_and_profile_gate_67a3a0")
    set_slot_refs = ref_source_set(rel32_refs, "set_save_slot_67a810")
    bc4_value_refs = ref_source_set(rel32_refs, "bc4_value_accessor_678f20")
    bc4_is_three_refs = ref_source_set(rel32_refs, "bc4_is_three_accessor_679f30")
    set_bc4_refs = ref_source_set(rel32_refs, "set_bc4_67a970")
    promote_bc4_refs = ref_source_set(rel32_refs, "promote_bc4_2_to_3_67a980")
    case2_notify_refs = ref_source_set(rel32_refs, "post_pump_case2_notify_810970")
    event_refs = ref_source_set(rel32_refs, "task_event_80dc10")
    task_wrapper_refs = ref_source_set(rel32_refs, "task_local_wrapper_7449e0")
    task_enqueue_refs = ref_source_set(rel32_refs, "task_enqueue_7a7b60")
    task_enqueue_link_refs = ref_source_set(rel32_refs, "task_enqueue_link_7a7bb0")
    builder_refs = ref_source_set(rel32_refs, "pump_owner_builder_828fd0")
    pump_owner_vtable_entries = read_qwords(data, image_base, sections, VTABLE_GROUP_BASES["pump_owner_vtable_7290"], 8)
    vtable_ref_targets = {ref["target_va"] for ref in rip_relative_vtable_refs}
    b5e_direct_sources = {str(access.get("source_va")) for access in game_man_b5e_context.get("direct_accesses", [])}
    b5e_getter_sources = {str(source) for source in game_man_b5e_context.get("getter_call_sources", [])}
    b5e_setter_sources = {str(source) for source in game_man_b5e_context.get("setter_call_sources", [])}
    b5e_bulk_clear_sources = {str(source) for source in game_man_b5e_context.get("bulk_clear_call_sources", [])}
    selected_field_sources = {
        name: {str(row.get("source_va")) for row in rows}
        for name, rows in slot_reset_selected_value_field_access_context.get("access_rows", {}).items()
    }

    evidence = {
        "exe_path": str(exe),
        "exe_md5": exe_md5,
        "image_base": f"0x{image_base:x}",
        "targets": {name: {"va": f"0x{addr:x}", "rva": f"0x{addr - image_base:x}"} for name, addr in TARGETS.items()},
        "rel32_refs": rel32_refs,
        "absolute_qword_refs": absolute_refs,
        "ghidra_reconciliation": ghidra_reconciliation,
        "menu_task_update_vtable_context": vtable_context,
        "game_man_field_windows": field_windows,
        "jump_tables": jump_tables,
        "switch_target_windows": switch_target_windows,
        "task_object_windows": task_object_windows,
        "menu_load_pump_handoff_context": menu_load_pump_handoff_context,
        "builder_callsite_windows": builder_callsite_windows,
        "function_ranges": function_ranges,
        "builder_argument_flows": builder_argument_flows,
        "builder_constructor_call_contexts": builder_constructor_call_contexts,
        "wrapper_thunk_call_contexts": wrapper_thunk_call_contexts,
        "outer_thunk_call_contexts": outer_thunk_call_contexts,
        "entry_thunk_call_contexts": entry_thunk_call_contexts,
        "entry_helper_vtable_contexts": entry_helper_vtable_contexts,
        "entry_vtable_rip_refs": entry_vtable_rip_refs,
        "selector_entry_vtable_rip_refs": selector_entry_vtable_rip_refs,
        "entry_family_descriptor_rip_refs": entry_family_descriptor_rip_refs,
        "entry_family_builder_call_contexts": entry_family_builder_call_contexts,
        "entry_selector_contexts": entry_selector_contexts,
        "entry_selector_parent_contexts": entry_selector_parent_contexts,
        "selector_parent_thunk_contexts": selector_parent_thunk_contexts,
        "selector_outer_thunk_contexts": selector_outer_thunk_contexts,
        "entry_level_helper_contexts": entry_level_helper_contexts,
        "selector_entry_helper_vtable_contexts": selector_entry_helper_vtable_contexts,
        "selector6_builder_context": selector6_builder_context,
        "selector6_builder_direct_caller_context": selector6_builder_direct_caller_context,
        "selector6_builder_entry_owner_context": selector6_builder_entry_owner_context,
        "selector6_owner_compose_parent_contexts": selector6_owner_compose_parent_contexts,
        "selector6_owner_variant_caller_contexts": selector6_owner_variant_caller_contexts,
        "continue_selector_dispatch_comparison": continue_selector_dispatch_comparison,
        "selector_owner_lifecycle_context": selector_owner_lifecycle_context,
        "selector_owner_factory_entry_context": selector_owner_factory_entry_context,
        "autoload_native_transition_candidates": summarize_autoload_native_transition_candidates(
            selector6_owner_compose_parent_contexts,
            selector6_owner_variant_caller_contexts,
            selector_owner_lifecycle_context,
            slot_reset_state_table_init_context,
            slot_reset_play_game_context,
            slot_reset_play_game_submit_context,
            set_save_slot_callsite_contexts,
        ),
        "selector_submit_context": selector_submit_context,
        "selector_final_enqueue_context": selector_final_enqueue_context,
        "selector_pair_builder_context": selector_pair_builder_context,
        "set_save_slot_callsite_contexts": set_save_slot_callsite_contexts,
        "game_man_b5e_context": game_man_b5e_context,
        "slot_reset_dispatch_context": slot_reset_dispatch_context,
        "slot_reset_title_parent_ctor_context": slot_reset_title_parent_ctor_context,
        "slot_reset_state_table_init_context": slot_reset_state_table_init_context,
        "slot_reset_to_menu_job_wait_context": slot_reset_to_menu_job_wait_context,
        "slot_reset_selected_value_field_access_context": slot_reset_selected_value_field_access_context,
        "slot_reset_selected_value_caller_context": slot_reset_selected_value_caller_context,
        "slot_reset_selected_value_context": slot_reset_selected_value_context,
        "slot_reset_play_game_submit_context": slot_reset_play_game_submit_context,
        "slot_reset_play_game_context": slot_reset_play_game_context,
        "slot_reset_title_queue_state_table_context": slot_reset_title_queue_state_table_context,
        "slot_reset_title_queue_producer_context": slot_reset_title_queue_producer_context,
        "slot_reset_menu_job_wait_context": slot_reset_menu_job_wait_context,
        "slot_reset_global_toggle_context": slot_reset_global_toggle_context,
        "slot_reset_timed_queue_context": slot_reset_timed_queue_context,
        "title_accept_payload_context": title_accept_payload_context,
        "slot_reset_end_flow_branch_gate_table_init_context": slot_reset_end_flow_branch_gate_table_init_context,
        "finish_gate_synchronization_context": finish_gate_synchronization_context,
        "slot_reset_end_flow_branch_gate_state_handlers_context": slot_reset_end_flow_branch_gate_state_handlers_context,
        "slot_reset_end_flow_branch_gate_context": slot_reset_end_flow_branch_gate_context,
        "slot_reset_end_flow_wait_context": slot_reset_end_flow_wait_context,
        "slot_reset_end_flow_tail_branch_context": slot_reset_end_flow_tail_branch_context,
        "entry_selector_window": entry_selector_window,
        "task_local_wrapper_contexts": task_local_wrapper_contexts,
        "task_local_wrapper_sequences": task_local_wrapper_sequences,
        "rip_relative_vtable_refs": rip_relative_vtable_refs,
        "pump_owner_vtable_entries": pump_owner_vtable_entries,
        "summary": {
            "menu_other_load_calls_map_load": any(ref["source_va"] == "0x14082bb09" for ref in rel32_refs["map_load_67bc10"]),
            "menu_load_wrappers_immediate_primitives_submit_state": bool(
                menu_load_pump_handoff_context.get("all_immediate_wrappers_call_primitives_and_submit_state")
            ),
            "menu_load_pump_update_maps_return_to_submit_state": bool(
                menu_load_pump_handoff_context.get("pump_update_submits_state_from_pump_return")
            ),
            "menu_load_pump_update_selects_delta_or_default_by_delay": bool(
                menu_load_pump_handoff_context.get("pump_update_selects_delta_or_default_by_delay")
            ),
            "menu_load_state_submit_helper_field_store_mapped": bool(
                menu_load_pump_handoff_context.get("state_submit_helper_is_simple_field_store")
            ),
            "menu_load_state_submit_helper_generic_fan_in_mapped": bool(
                menu_load_pump_handoff_context.get("state_submit_helper_is_generic_broad_fan_in")
            ),
            "menu_load_state_submit_sources_exact_subset_mapped": bool(
                menu_load_pump_handoff_context.get("menu_load_state_submit_sources_are_exact_subset")
            ),
            "menu_load_state_helper_family_bodies_mapped": bool(
                menu_load_pump_handoff_context.get("state_helper_family_bodies_mapped")
            ),
            "menu_load_state_helper_family_ref_counts_mapped": bool(
                menu_load_pump_handoff_context.get("state_helper_family_ref_counts_mapped")
            ),
            "menu_load_state_helper_family_timed_queue_consumer_mapped": bool(
                menu_load_pump_handoff_context.get("state_helper_family_timed_queue_consumer_mapped")
            ),
            "menu_load_state_terminal_compare_consumers_mapped": bool(
                menu_load_pump_handoff_context.get("state_code_terminal_compare_consumers_mapped")
            ),
            "menu_load_state_empty_check_consumer_mapped": bool(
                menu_load_pump_handoff_context.get("state_code_empty_check_consumer_mapped")
            ),
            "menu_load_state_pending_success_failure_semantics_mapped": bool(
                menu_load_pump_handoff_context.get("state_code_pending_success_failure_semantics_mapped")
            ),
            "menu_load_state_consumer_vtables_mapped": bool(
                menu_load_pump_handoff_context.get("state_code_consumer_vtables_mapped")
            ),
            "menu_load_state_consumer_roles_classified": bool(
                menu_load_pump_handoff_context.get("state_code_consumer_roles_classified")
            ),
            "menu_load_state_consumer_constructor_callers_mapped": bool(
                menu_load_pump_handoff_context.get("state_code_consumer_constructor_callers_mapped")
            ),
            "pump_wrapper_calls_delta_pump": any(ref["source_va"] == "0x14082a106" for ref in rel32_refs["save_load_pump_delta"]),
            "pump_wrapper_calls_default_pump": any(ref["source_va"] == "0x14082a10d" for ref in rel32_refs["save_load_pump_default"]),
            "pump_wrapper_has_vtable_entry": bool(pump_abs_refs),
            "move_map_scheduler_calls_dispatch": bool(rel32_refs["move_map_save_load_dispatch"]),
            "dispatcher_calls_requested_slot_validation": any(ref["source_va"] == "0x140afba3d" for ref in rel32_refs["requested_slot_validation"]),
            "dispatcher_calls_all_queue_helpers": all(
                any(ref["source_va"].startswith("0x140afb") for ref in rel32_refs[name])
                for name in ["combined_load_67b940", "continue_load_67b750", "current_slot_load_67b570"]
            ),
            "b72_accessor_reads_b72_and_bc4": {"b72_save_request_profile", "bc4_queue_gate"}.issubset(
                fields_seen(field_windows, "b72_profile_flag_accessor_6793d0")
            ),
            "b73_accessor_reads_b73_and_bc4": {"b73_save_request", "bc4_queue_gate"}.issubset(
                fields_seen(field_windows, "b73_request_flag_accessor_679370")
            ),
            "b78_accessor_reads_requested_slot_index": "b78_requested_slot_index" in fields_seen(
                field_windows, "b78_requested_slot_accessor_6793c0"
            ),
            "b75_accessor_reads_load_arg": "b75_load_arg" in fields_seen(field_windows, "b75_load_arg_accessor_679100"),
            "save_load_pumps_touch_transition_fields": all(
                {"b80_load_state", "bb8_pending_transition", "bc0_transition_count"}.issubset(fields_seen(field_windows, window))
                for window in ["pump_delta_6794b0", "pump_default_679510"]
            ),
            "title_menu_continue_calls_request_save": "0x140af7acd" in request_save_refs,
            "move_map_continue_calls_request_save_and_profile": {"0x140afab5f"}.issubset(request_save_refs)
            and {"0x140afab6a"}.issubset(save_profile_refs),
            "menu_set_slot_wrapper_calls_set_save_slot": "0x14082c379" in set_slot_refs,
            "gate_function_sets_b72_b73_bc4": {
                "b72_save_request_profile",
                "b73_save_request",
                "bc4_queue_gate",
            }.issubset(fields_seen(field_windows, "save_request_gate_67a3a0"))
            if "save_request_gate_67a3a0" in field_windows
            else False,
            "gate_function_has_known_callers": {"0x14059d90e", "0x1407a36b0", "0x1407a3760"}.issubset(gate_refs),
            "bc4_value_accessor_called_from_move_map": "0x140af840e" in bc4_value_refs,
            "bc4_is_three_accessor_has_task_callers": {"0x1407a3144", "0x1407a37e4"}.issubset(bc4_is_three_refs),
            "set_bc4_called_from_move_map": "0x140afb7fd" in set_bc4_refs,
            "promote_bc4_after_save_load_pump": {"0x140afbb17", "0x140afbbd0"}.issubset(promote_bc4_refs),
            "bc4_windows_touch_bc4": all(
                "bc4_queue_gate" in fields_seen(field_windows, window)
                for window in [
                    "bc4_value_accessor_678f20",
                    "bc4_is_three_accessor_679f30",
                    "set_bc4_67a970",
                    "promote_bc4_2_to_3_67a980",
                ]
            ),
            "post_pump_switch_table_has_ten_cases": len(jump_tables.get("post_pump_default_switch_afbd04", [])) == 10,
            "post_pump_return_zero_promotes_bc4": any(
                row["case"] == 0 and row["target_va"] == "0x140afbb17"
                for row in jump_tables.get("post_pump_default_switch_afbd04", [])
            ),
            "post_pump_return_one_marks_completion": any(
                row["case"] == 1 and row["target_va"] == "0x140afbb71"
                for row in jump_tables.get("post_pump_default_switch_afbd04", [])
            ),
            "post_pump_returns_three_seven_nine_promote_bc4": all(
                any(row["case"] == case and row["target_va"] == "0x140afbbc6" for row in jump_tables.get("post_pump_default_switch_afbd04", []))
                for case in [3, 7, 9]
            ),
            "switch_case0_calls_bc4_promoter": "0x140afbb17" in promote_bc4_refs,
            "switch_case1_checks_task_and_sets_completion_flag": bool(
                switch_target_windows.get("case1_completion_flag", {}).get("checks_task_12a")
                and switch_target_windows.get("case1_completion_flag", {}).get("sets_global_82a8")
            ),
            "switch_case2_calls_notify_and_event_reset": "0x140afbb5b" in case2_notify_refs and "0x140afbb6c" in event_refs,
            "switch_case8_loads_context_global": bool(switch_target_windows.get("case8_context_path", {}).get("loads_case8_context_global")),
            "pump_owner_vtable_constructor_sets_base": any(
                ref["source_va"] == "0x140827555" and ref["target_va"] == "0x142ac7290"
                for ref in rip_relative_vtable_refs
            ),
            "pump_owner_vtable_update_entry_is_82a0f0": len(pump_owner_vtable_entries) > 2
            and pump_owner_vtable_entries[2]["value_va"] == "0x14082a0f0",
            "pump_owner_vtable_name_entry_is_82c170": len(pump_owner_vtable_entries) > 3
            and pump_owner_vtable_entries[3]["value_va"] == "0x14082c170",
            "vtable_family_has_all_related_bases": all(
                f"0x{base:x}" in vtable_ref_targets for base in VTABLE_GROUP_BASES.values()
            ),
            "pump_owner_update_reads_task_plus8_float": bool(
                task_object_windows.get("pump_owner_update_82a0f0", {}).get("loads_task_plus8_float")
            ),
            "pump_owner_update_branches_on_task_plus8": bool(
                task_object_windows.get("pump_owner_update_82a0f0", {}).get("compares_plus8_to_zero")
                and task_object_windows.get("pump_owner_update_82a0f0", {}).get("branches_to_default_on_nonpositive")
            ),
            "pump_owner_clone_preserves_task_plus8_float": bool(
                task_object_windows.get("pump_owner_clone_82b3a0", {}).get("loads_task_plus8_float")
                and task_object_windows.get("pump_owner_clone_82b3a0", {}).get("stores_task_plus8_float")
            ),
            "pump_owner_task_plus8_controls_pump_choice": bool(
                task_object_windows.get("pump_owner_update_82a0f0", {}).get("loads_task_plus8_float")
                and any(ref["source_va"] == "0x14082a106" for ref in rel32_refs["save_load_pump_delta"])
                and any(ref["source_va"] == "0x14082a10d" for ref in rel32_refs["save_load_pump_default"])
            ),
            "pump_owner_local_builder_writes_vtable": any(
                ref["source_va"] == "0x14082903c" and ref["target_va"] == "0x142ac7290"
                for ref in rip_relative_vtable_refs
            ),
            "pump_owner_local_builder_stores_xmm2_to_plus8": bool(
                task_object_windows.get("pump_owner_local_builder_829000", {}).get("stores_xmm2_to_local_task_plus8")
            ),
            "pump_owner_local_builder_calls_task_wrapper": "0x14082905c" in task_wrapper_refs,
            "pump_owner_local_builder_calls_task_enqueue": "0x140829069" in task_enqueue_refs,
            "pump_owner_local_builder_calls_task_enqueue_link": "0x140829088" in task_enqueue_link_refs,
            "pump_owner_builder_has_four_callers": {
                "0x140824a14",
                "0x140824aef",
                "0x14082514f",
                "0x14082584f",
            }.issubset(builder_refs),
            "builder_positive_delay_call_uses_300s": bool(
                builder_callsite_windows.get("positive_delay_ba30", {}).get("calls_builder")
                and builder_callsite_windows.get("positive_delay_ba30", {}).get("xmm2_value") == 300.0
            ),
            "builder_menu_wrappers_cover_continue_new_other": {
                "0x14082bac0",
                "0x14082ba80",
                "0x14082bb00",
            }.issubset(
                {
                    str(window.get("wrapper_target_va"))
                    for key, window in builder_callsite_windows.items()
                    if key != "positive_delay_ba30"
                }
            ),
            "builder_continue_new_other_use_zero_xmm2": all(
                builder_callsite_windows.get(key, {}).get("xmm2_value") == 0.0
                for key in ["other_load_bb00_zero", "new_or_load_ba80_zero", "continue_bac0_zero"]
            ),
            "builder_continue_new_other_call_builder": all(
                builder_callsite_windows.get(key, {}).get("calls_builder") is True
                for key in ["other_load_bb00_zero", "new_or_load_ba80_zero", "continue_bac0_zero"]
            ),
            "zero_xmm2_menu_wrappers_select_default_pump": bool(
                all(
                    builder_callsite_windows.get(key, {}).get("xmm2_value") == 0.0
                    for key in ["other_load_bb00_zero", "new_or_load_ba80_zero", "continue_bac0_zero"]
                )
                and task_object_windows.get("pump_owner_update_82a0f0", {}).get("branches_to_default_on_nonpositive")
                and any(ref["source_va"] == "0x14082a10d" for ref in rel32_refs["save_load_pump_default"])
            ),
            "positive_xmm2_wrapper_selects_delta_pump": bool(
                builder_callsite_windows.get("positive_delay_ba30", {}).get("xmm2_value") == 300.0
                and any(ref["source_va"] == "0x14082a106" for ref in rel32_refs["save_load_pump_delta"])
            ),
            "builder_callers_have_expected_function_ranges": all(
                function_ranges.get(name, {}).get("begin_va") == expected_begin
                for name, expected_begin in {
                    "positive_delay_builder_caller": "0x1408249a0",
                    "other_load_builder_caller": "0x140824a80",
                    "new_or_load_builder_caller": "0x1408250e0",
                    "continue_builder_caller": "0x1408257e0",
                }.items()
            ),
            "menu_wrappers_have_expected_function_ranges": all(
                function_ranges.get(name, {}).get("begin_va") == expected_begin
                for name, expected_begin in {
                    "positive_delay_wrapper_ba30": "0x14082ba30",
                    "new_or_load_wrapper_ba80": "0x14082ba80",
                    "continue_wrapper_bac0": "0x14082bac0",
                    "other_load_wrapper_bb00": "0x14082bb00",
                }.items()
            ),
            "pump_owner_builder_range_contains_local_builder": bool(
                function_ranges.get("pump_owner_builder", {}).get("begin_va") == "0x140828fd0"
                and function_ranges.get("pump_owner_builder", {}).get("end_va") == "0x1408291b3"
            ),
            "builder_rcx_flows_from_caller_rdx": all(
                builder_argument_flows.get(name, {}).get("rcx_source") == "incoming_rdx_via_rbx"
                for name in BUILDER_CALLER_SITES
            ),
            "builder_rdx_is_stack_task_descriptor": all(
                builder_argument_flows.get(name, {}).get("rdx_source") == "stack_descriptor_r11_minus_0x50"
                for name in BUILDER_CALLER_SITES
            ),
            "zero_xmm2_callers_share_scheduler_argument_flow": all(
                builder_argument_flows.get(name, {}).get("rcx_source") == "incoming_rdx_via_rbx"
                and builder_argument_flows.get(name, {}).get("rdx_source") == "stack_descriptor_r11_minus_0x50"
                and builder_argument_flows.get(name, {}).get("xmm2_source") == "zero"
                for name in ["other_load_builder_caller", "new_or_load_builder_caller", "continue_builder_caller"]
            ),
            "zero_xmm2_wrapper_constructors_have_callers": all(
                len(builder_constructor_call_contexts.get(name, [])) > 0
                for name in [
                    "other_load_builder_caller_824a80",
                    "new_or_load_builder_caller_8250e0",
                    "continue_builder_caller_8257e0",
                ]
            ),
            "zero_xmm2_wrapper_constructor_callers_have_pdata": all(
                all(context.get("function_begin_va") is not None for context in builder_constructor_call_contexts.get(name, []))
                for name in [
                    "other_load_builder_caller_824a80",
                    "new_or_load_builder_caller_8250e0",
                    "continue_builder_caller_8257e0",
                ]
            ),
            "positive_and_zero_wrapper_constructor_refs_mapped": all(
                len(builder_constructor_call_contexts.get(name, [])) > 0 for name in BUILDER_CONSTRUCTOR_TARGETS
            ),
            "zero_xmm2_thunks_have_callers": all(
                len(wrapper_thunk_call_contexts.get(name, [])) > 0
                for name in ["other_load_thunk_822020", "new_or_load_thunk_8221b0", "continue_thunk_822300"]
            ),
            "zero_xmm2_thunk_callers_have_pdata": all(
                all(context.get("function_begin_va") is not None for context in wrapper_thunk_call_contexts.get(name, []))
                for name in ["other_load_thunk_822020", "new_or_load_thunk_8221b0", "continue_thunk_822300"]
            ),
            "positive_and_zero_thunk_refs_mapped": all(
                len(wrapper_thunk_call_contexts.get(name, [])) > 0 for name in WRAPPER_THUNK_TARGETS
            ),
            "zero_xmm2_outer_thunks_have_callers": all(
                len(outer_thunk_call_contexts.get(name, [])) > 0
                for name in ["other_load_outer_823550", "new_or_load_outer_8236d0", "continue_outer_823810"]
            ),
            "zero_xmm2_outer_thunk_callers_have_pdata": all(
                all(context.get("function_begin_va") is not None for context in outer_thunk_call_contexts.get(name, []))
                for name in ["other_load_outer_823550", "new_or_load_outer_8236d0", "continue_outer_823810"]
            ),
            "positive_and_zero_outer_thunk_refs_mapped": all(
                len(outer_thunk_call_contexts.get(name, [])) > 0 for name in OUTER_THUNK_TARGETS
            ),
            "zero_xmm2_entry_thunks_have_callers": all(
                len(entry_thunk_call_contexts.get(name, [])) > 0
                for name in ["other_load_entry_822810", "new_or_load_entry_8229f0", "continue_entry_822b30"]
            ),
            "zero_xmm2_entry_thunk_callers_have_pdata": all(
                all(context.get("function_begin_va") is not None for context in entry_thunk_call_contexts.get(name, []))
                for name in ["other_load_entry_822810", "new_or_load_entry_8229f0", "continue_entry_822b30"]
            ),
            "entry_thunk_callers_supply_task_plus8_as_r8": all(
                all(context.get("moves_rcx_plus8_to_r8") for context in entry_thunk_call_contexts.get(name, []))
                for name in ENTRY_THUNK_TARGETS
            ),
            "positive_and_zero_entry_thunk_refs_mapped": all(
                len(entry_thunk_call_contexts.get(name, [])) > 0 for name in ENTRY_THUNK_TARGETS
            ),
            "entry_helpers_have_absolute_vtable_refs": all(
                len(entry_helper_vtable_contexts.get(name, [])) > 0 for name in ENTRY_HELPER_TARGETS
            ),
            "zero_xmm2_entry_helpers_have_vtable_refs": all(
                len(entry_helper_vtable_contexts.get(name, [])) > 0
                for name in [
                    "other_load_entry_helper_829d90",
                    "new_or_load_entry_helper_82a000",
                    "continue_entry_helper_82a270",
                ]
            ),
            "entry_helper_refs_are_rdata_vtable_slots": all(
                all(context.get("section") == ".rdata" for context in entry_helper_vtable_contexts.get(name, []))
                for name in ENTRY_HELPER_TARGETS
            ),
            "continue_entry_helper_vtable_slot_is_142ac7888": any(
                context.get("source_va") == "0x142ac7888"
                for context in entry_helper_vtable_contexts.get("continue_entry_helper_82a270", [])
            ),
            "other_and_new_entry_helpers_share_neighbor_vtables": bool(
                any(context.get("source_va") == "0x142ac7930" for context in entry_helper_vtable_contexts.get("other_load_entry_helper_829d90", []))
                and any(context.get("source_va") == "0x142ac78f8" for context in entry_helper_vtable_contexts.get("new_or_load_entry_helper_82a000", []))
            ),
            "entry_helper_vtable_layout_is_base_plus_0x10": all(
                all(context.get("helper_matches_target") is True for context in entry_helper_vtable_contexts.get(name, []))
                for name in ENTRY_HELPER_TARGETS
            ),
            "entry_helper_vtables_have_task_plus8_accessors": all(
                all(context.get("task_plus8_accessor_returns_rcx_plus8") is True for context in entry_helper_vtable_contexts.get(name, []))
                for name in ENTRY_HELPER_TARGETS
            ),
            "entry_helper_constructors_store_vtable_base": all(
                all(context.get("constructor_stores_vtable_base") is True for context in entry_helper_vtable_contexts.get(name, []))
                for name in ENTRY_HELPER_TARGETS
            ),
            "entry_helper_copy_or_clone_entries_store_vtable_base": all(
                all(context.get("copy_or_clone_stores_vtable_base") is True for context in entry_helper_vtable_contexts.get(name, []))
                for name in ENTRY_HELPER_TARGETS
            ),
            "continue_vtable_roles_decoded": any(
                context.get("vtable_base_va") == "0x142ac7878"
                and context.get("task_plus8_accessor_va") == "0x14082a670"
                and context.get("constructor_va") == "0x140827660"
                and context.get("copy_or_clone_va") == "0x14082b590"
                and context.get("helper_va") == "0x14082a270"
                for context in entry_helper_vtable_contexts.get("continue_entry_helper_82a270", [])
            ),
            "entry_vtable_bases_have_three_rip_refs": all(
                len(entry_vtable_rip_refs.get(name, [])) == 3 for name in ENTRY_HELPER_VTABLE_BASES
            ),
            "continue_vtable_refs_are_ctor_dtor_clone": {
                "0x140827685",
                "0x1408299d2",
                "0x14082b5b5",
            } == {str(ref.get("source_va")) for ref in entry_vtable_rip_refs.get("continue_entry_vtable_7878", [])},
            "other_vtable_refs_are_ctor_dtor_clone": {
                "0x1408272b5",
                "0x140829532",
                "0x14082b055",
            } == {str(ref.get("source_va")) for ref in entry_vtable_rip_refs.get("other_load_entry_vtable_7920", [])},
            "entry_task_ctor_copy_have_no_direct_rel32_callers": all(
                len(rel32_refs.get(name, [])) == 0 for name in ENTRY_TASK_CTOR_COPY_TARGET_NAMES
            ),
            "menu_region_task_local_wrapper_contexts_mapped": len(task_local_wrapper_contexts) >= 8,
            "menu_region_task_local_wrapper_contexts_have_enqueue": sum(
                1 for context in task_local_wrapper_contexts if context.get("nearby_enqueue_calls")
            ) >= 6,
            "menu_region_task_local_wrapper_contexts_have_vtable_leas": sum(
                1 for context in task_local_wrapper_contexts if context.get("nearby_rip_lea_vtables")
            ) >= 4,
            "pump_owner_local_wrapper_sequence_has_link_enqueue": any(
                context.get("source_va") == "0x14082905c"
                and any(call.get("source_va") == "0x140829069" for call in context.get("nearby_enqueue_calls", []))
                for context in task_local_wrapper_contexts
            )
            and any(
                context.get("source_va") == "0x140829078"
                and any(call.get("source_va") == "0x140829088" for call in context.get("nearby_enqueue_calls", []))
                for context in task_local_wrapper_contexts
            ),
            "task_local_wrapper_sequences_grouped_by_function": len(task_local_wrapper_sequences) >= 4,
            "pump_owner_sequence_descriptor_and_enqueue_decoded": bool(
                set(task_local_wrapper_sequences.get("0x140828fd0", {}).get("descriptor_vtables", [])) == {"0x142ac7290"}
                and {"0x140829069", "0x140829088", "0x140829094"}.issubset(
                    {str(call.get("source_va")) for call in task_local_wrapper_sequences.get("0x140828fd0", {}).get("enqueue_calls", [])}
                )
            ),
            "entry_family_8279e0_sequence_descriptor_and_enqueue_decoded": bool(
                set(task_local_wrapper_sequences.get("0x1408279e0", {}).get("descriptor_vtables", [])) == {"0x142ac73a8"}
                and {"0x140827a79", "0x140827a98", "0x140827aa4"}.issubset(
                    {str(call.get("source_va")) for call in task_local_wrapper_sequences.get("0x1408279e0", {}).get("enqueue_calls", [])}
                )
            ),
            "entry_family_and_pump_owner_share_enqueue_shape": bool(
                len(task_local_wrapper_sequences.get("0x1408279e0", {}).get("enqueue_calls", [])) >= 3
                and len(task_local_wrapper_sequences.get("0x140828fd0", {}).get("enqueue_calls", [])) >= 3
                and any(
                    call.get("target_name") == "task_enqueue_link_7a7bb0"
                    for call in task_local_wrapper_sequences.get("0x1408279e0", {}).get("enqueue_calls", [])
                )
                and any(
                    call.get("target_name") == "task_enqueue_link_7a7bb0"
                    for call in task_local_wrapper_sequences.get("0x140828fd0", {}).get("enqueue_calls", [])
                )
            ),
            "entry_family_builder_has_single_callsite": {
                "0x140824944"
            } == {str(ref.get("source_va")) for ref in rel32_refs.get("entry_family_builder_8279e0", [])},
            "entry_family_builder_call_uses_300s_and_ba00_wrapper": any(
                context.get("source_va") == "0x140824944"
                and context.get("wrapper_target_va") == "0x14082ba00"
                and context.get("xmm2_value") == 300.0
                and context.get("moves_rbx_to_rcx_before_call")
                and context.get("uses_stack_descriptor_as_rdx_before_call")
                for context in entry_family_builder_call_contexts
            ),
            "entry_family_descriptor_vtable_has_four_refs": len(
                entry_family_descriptor_rip_refs.get("entry_family_descriptor_vtable_73a8", [])
            ) == 4,
            "entry_family_descriptor_refs_include_builder_lifecycle": {
                "0x1408277a5",
                "0x140827a4c",
                "0x140829b92",
                "0x14082b7d5",
            } == {
                str(ref.get("source_va"))
                for ref in entry_family_descriptor_rip_refs.get("entry_family_descriptor_vtable_73a8", [])
            },
            "entry_selector_has_two_known_callers": {"0x140824984", "0x140824b34"}.issubset(
                {str(context.get("source_va")) for context in entry_selector_contexts}
            ),
            "entry_selector_callers_cover_selector_7_and_1": {1, 7}.issubset(
                {int(context.get("r8_immediate")) for context in entry_selector_contexts if context.get("r8_immediate") is not None}
            ),
            "entry_selector_writes_selector_to_descriptor": bool(
                entry_selector_window.get("writes_r8d_to_descriptor_plus4") and entry_selector_window.get("branches_on_selector_minus_one_le_one")
            ),
            "entry_selector_uses_two_alloc_paths_and_enqueue": bool(
                entry_selector_window.get("has_alloc_high_path")
                and entry_selector_window.get("has_alloc_low_path")
                and {"0x140824b94", "0x140824baf"}.issubset(task_enqueue_refs)
            ),
            "entry_selector_parent_thunks_have_single_callers": all(
                len(entry_selector_parent_contexts.get(name, [])) == 1 for name in ENTRY_SELECTOR_TARGET_NAMES
            ),
            "entry_selector_parent_thunks_forward_r8_plus_0x50": all(
                all(
                    context.get("forwards_r8_plus_0x50_or_null")
                    and context.get("forwards_rcx_to_rdx")
                    and context.get("forwards_r9_to_rcx")
                    for context in entry_selector_parent_contexts.get(name, [])
                )
                for name in ENTRY_SELECTOR_TARGET_NAMES
            ),
            "selector6_parent_thunk_adjacent_to_continue_zero_thunk": any(
                context.get("source_va") == "0x1408222e3" and context.get("function_begin_va") == "0x1408222b0"
                for context in entry_selector_parent_contexts.get("selector6_entry_func_8257a0", [])
            ),
            "entry_selector_entry_functions_are_thin_selector_wrappers": all(
                all(
                    context.get("entry_function_calls_entry_selector") and not context.get("entry_function_has_rip_vtable_lea")
                    for context in [c for c in entry_selector_contexts if c.get("function_begin_va") == f"0x{TARGETS[name]:x}"]
                )
                for name in ENTRY_SELECTOR_TARGET_NAMES
            ),
            "selector6_entry_has_no_direct_descriptor_vtable_refs": any(
                context.get("function_begin_va") == "0x1408257a0" and not context.get("entry_function_has_rip_vtable_lea")
                for context in entry_selector_contexts
            ),
            "selector6_entry_immediately_precedes_continue_builder": any(
                context.get("function_begin_va") == "0x1408257a0"
                and context.get("function_end_va") == "0x1408257dc"
                for context in entry_selector_contexts
            ),
            "selector_parent_thunks_have_single_outer_callers": all(
                len(selector_parent_thunk_contexts.get(name, [])) == 1 for name in ENTRY_SELECTOR_PARENT_TARGET_NAMES
            ),
            "selector6_parent_outer_context_is_8237d0": any(
                context.get("source_va") == "0x1408237ee" and context.get("function_begin_va") == "0x1408237d0"
                for context in selector_parent_thunk_contexts.get("selector6_parent_thunk_8222b0", [])
            ),
            "selector_outer_thunks_have_single_entry_callers": all(
                len(selector_outer_thunk_contexts.get(name, [])) == 1 for name in ENTRY_SELECTOR_OUTER_TARGET_NAMES
            ),
            "selector6_outer_entry_context_is_822af0": any(
                context.get("source_va") == "0x140822b14"
                and context.get("function_begin_va") == "0x140822af0"
                and context.get("forwards_r9_to_r8")
                and context.get("forwards_r8_to_rdx")
                for context in selector_outer_thunk_contexts.get("selector6_outer_thunk_8237d0", [])
            ),
            "selector6_outer_and_continue_outer_are_adjacent": bool(
                any(
                    context.get("source_va") == "0x140822b14" and context.get("function_begin_va") == "0x140822af0"
                    for context in selector_outer_thunk_contexts.get("selector6_outer_thunk_8237d0", [])
                )
                and "0x140822b51" in ref_source_set(rel32_refs, "continue_outer_823810")
            ),
            "selector6_entry_level_has_single_helper_caller": any(
                context.get("source_va") == "0x14082a21d" and context.get("function_begin_va") == "0x14082a1f0"
                for context in entry_level_helper_contexts.get("selector6_entry_level_822af0", [])
            )
            and len(entry_level_helper_contexts.get("selector6_entry_level_822af0", [])) == 1,
            "continue_entry_level_has_single_continue_helper_caller": any(
                context.get("source_va") == "0x14082a29a" and context.get("function_begin_va") == "0x14082a270"
                for context in entry_level_helper_contexts.get("continue_entry_822b30", [])
            )
            and len(entry_level_helper_contexts.get("continue_entry_822b30", [])) == 1,
            "selector6_entry_helper_supplies_task_plus8_and_selector_arg": any(
                context.get("source_va") == "0x14082a21d"
                and context.get("passes_incoming_r8_to_r9")
                and context.get("passes_task_plus8_as_r8")
                and context.get("passes_stack_flag_as_edx")
                and context.get("moves_incoming_rdx_to_rbx")
                and context.get("calls_with_rbx_as_rcx")
                for context in entry_level_helper_contexts.get("selector6_entry_level_822af0", [])
            ),
            "continue_entry_helper_supplies_task_plus8_without_selector_arg": any(
                context.get("source_va") == "0x14082a29a"
                and not context.get("passes_incoming_r8_to_r9")
                and context.get("passes_task_plus8_as_r8")
                and context.get("passes_stack_flag_as_edx")
                and context.get("moves_incoming_rdx_to_rbx")
                and context.get("calls_with_rbx_as_rcx")
                for context in entry_level_helper_contexts.get("continue_entry_822b30", [])
            ),
            "selector6_entry_helper_has_distinct_rdata_vtable_slot": any(
                context.get("source_va") == "0x142ac75b0"
                and context.get("vtable_base_va") == "0x142ac75a0"
                and context.get("helper_matches_target")
                for context in selector_entry_helper_vtable_contexts.get("selector6_entry_helper_82a1f0", [])
            ),
            "selector6_helper_vtable_distinct_from_continue_vtable": any(
                context.get("vtable_base_va") == "0x142ac75a0"
                for context in selector_entry_helper_vtable_contexts.get("selector6_entry_helper_82a1f0", [])
            )
            and any(
                context.get("vtable_base_va") == "0x142ac7878"
                for context in entry_helper_vtable_contexts.get("continue_entry_helper_82a270", [])
            ),
            "selector6_vtable_roles_decoded": any(
                context.get("vtable_base_va") == "0x142ac75a0"
                and context.get("task_plus8_accessor_va") == "0x14082a4f0"
                and context.get("constructor_va") == "0x140827610"
                and context.get("copy_or_clone_va") == "0x14082b4e0"
                and context.get("helper_va") == "0x14082a1f0"
                and context.get("destructor_va") == "0x140829940"
                and context.get("task_plus8_accessor_returns_rcx_plus8")
                and context.get("constructor_stores_vtable_base")
                and context.get("copy_or_clone_stores_vtable_base")
                and context.get("destructor_resets_vtable_base")
                for context in selector_entry_helper_vtable_contexts.get("selector6_entry_helper_82a1f0", [])
            ),
            "selector6_vtable_refs_are_lifecycle_plus_builder": {
                "0x140827635",
                "0x140827cab",
                "0x140829952",
                "0x14082b505",
            }
            == {str(ref.get("source_va")) for ref in selector_entry_vtable_rip_refs.get("selector6_entry_vtable_75a0", [])},
            "selector6_vtable_extra_ref_is_builder_context": any(
                ref.get("source_va") == "0x140827cab" and ref.get("function_begin_va") == "0x140827bd0"
                for ref in selector_entry_vtable_rip_refs.get("selector6_entry_vtable_75a0", [])
            ),
            "selector6_and_continue_vtable_roles_share_shape": any(
                context.get("vtable_base_va") == "0x142ac75a0"
                and context.get("task_plus8_accessor_returns_rcx_plus8")
                and context.get("constructor_stores_vtable_base")
                and context.get("copy_or_clone_stores_vtable_base")
                for context in selector_entry_helper_vtable_contexts.get("selector6_entry_helper_82a1f0", [])
            )
            and any(
                context.get("vtable_base_va") == "0x142ac7878"
                and context.get("task_plus8_accessor_returns_rcx_plus8")
                and context.get("constructor_stores_vtable_base")
                and context.get("copy_or_clone_stores_vtable_base")
                for context in entry_helper_vtable_contexts.get("continue_entry_helper_82a270", [])
            ),
            "selector6_builder_context_range_mapped": bool(
                selector6_builder_context.get("function_begin_va") == "0x140827bd0"
                and selector6_builder_context.get("function_end_va") == "0x14082826f"
            ),
            "selector6_builder_uses_incoming_scheduler_args": bool(
                selector6_builder_context.get("uses_incoming_r8_scratch")
                and selector6_builder_context.get("moves_incoming_rdx_to_rbx")
                and selector6_builder_context.get("moves_incoming_rcx_to_r13")
                and selector6_builder_context.get("loads_incoming_rdx_plus_0x30")
            ),
            "selector6_builder_constructs_six_descriptor_vtables": set(
                selector6_builder_context.get("descriptor_vtables", [])
            )
            == set(SELECTOR_BUILDER_DESCRIPTOR_VTABLES),
            "selector6_builder_calls_local_wrappers_for_five_tags": bool(
                len(selector6_builder_context.get("calls_by_target", {}).get("selector_builder_local_wrapper_744d10", [])) == 5
                and set(selector6_builder_context.get("local_wrapper_tags", [])) == set(SELECTOR_BUILDER_WRAPPER_TAGS)
            ),
            "selector6_builder_chain_appends_six_descriptors": bool(
                len(selector6_builder_context.get("calls_by_target", {}).get("selector_builder_chain_key_7a91e0", [])) == 6
                and len(selector6_builder_context.get("calls_by_target", {}).get("selector_builder_chain_append_7ccbb0", [])) == 6
            ),
            "selector6_builder_appends_selector6_descriptor_last": bool(
                selector6_builder_context.get("chain_append_order")
                and selector6_builder_context.get("chain_append_order", [])[-1].get("descriptor_name") == "selector6_entry_vtable_75a0"
            ),
            "selector6_builder_submits_chain_to_incoming_rcx_owner": bool(
                selector6_builder_context.get("calls_by_target", {}).get("selector_builder_chain_submit_78dac0") == ["0x14082805c"]
                and selector6_builder_context.get("submit_uses_incoming_rcx_owner")
            ),
            "selector6_builder_distinct_from_continue_builder_path": bool(
                any(
                    ref.get("function_begin_va") == "0x140827bd0"
                    for ref in selector_entry_vtable_rip_refs.get("selector6_entry_vtable_75a0", [])
                )
                and not any(
                    ref.get("function_begin_va") == "0x140827bd0"
                    for ref in entry_vtable_rip_refs.get("continue_entry_vtable_7878", [])
                )
            ),
            "selector6_builder_context_has_single_direct_caller": {
                "0x140825fb1"
            } == {str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_context_827bd0", [])},
            "selector6_builder_direct_caller_range_mapped": bool(
                selector6_builder_direct_caller_context.get("function_begin_va") == "0x140825f70"
                and selector6_builder_direct_caller_context.get("function_end_va") == "0x1408260ad"
            ),
            "selector6_builder_direct_caller_has_global_fast_path_gate": bool(
                selector6_builder_direct_caller_context.get("global_fast_path_flag_va") == "0x143d6cd80"
            ),
            "selector6_builder_direct_fast_path_arg_shuffle_mapped": bool(
                selector6_builder_direct_caller_context.get("captures_incoming_r8_in_rsi")
                and selector6_builder_direct_caller_context.get("captures_incoming_rdx_in_rbx")
                and selector6_builder_direct_caller_context.get("captures_incoming_rcx_in_rdi")
                and selector6_builder_direct_caller_context.get("direct_call_arg_shuffle", {}).get("rcx") == "incoming_rdx_via_rbx"
                and selector6_builder_direct_caller_context.get("direct_call_arg_shuffle", {}).get("rdx") == "incoming_r8_via_rsi"
                and selector6_builder_direct_caller_context.get("direct_call_arg_shuffle", {}).get("r8") == "incoming_rcx"
            ),
            "selector6_builder_direct_fallback_reconstructs_inputs": bool(
                selector6_builder_direct_caller_context.get("fallback_reads_incoming_rcx_plus_b0")
                and selector6_builder_direct_caller_context.get("fallback_reads_incoming_rcx_plus_70_via_rdi")
                and selector6_builder_direct_caller_context.get("fallback_passes_stack_clones_to_compose")
            ),
            "selector6_builder_direct_fallback_composes_and_enqueues": bool(
                selector6_builder_direct_caller_context.get("calls_by_target", {}).get("selector_builder_fallback_compose_828570") == [
                    "0x140826039"
                ]
                and selector6_builder_direct_caller_context.get("calls_by_target", {}).get("task_enqueue_7a7b60") == [
                    "0x140826045"
                ]
            ),
            "selector6_builder_parent_wrapper_arg_swap_mapped": bool(
                selector6_builder_direct_caller_context.get("parent_context", {}).get("source_va") == "0x140822487"
                and selector6_builder_direct_caller_context.get("parent_context", {}).get("function_begin_va") == "0x140822460"
                and selector6_builder_direct_caller_context.get("parent_passes_incoming_rdx_as_child_rcx")
                and selector6_builder_direct_caller_context.get("parent_passes_incoming_rcx_as_child_rdx")
            ),
            "selector6_builder_outer_entry_chain_mapped": bool(
                selector6_builder_direct_caller_context.get("outer_context", {}).get("source_va") == "0x14082396e"
                and selector6_builder_direct_caller_context.get("entry_context", {}).get("source_va") == "0x140822c94"
                and selector6_builder_direct_caller_context.get("outer_preserves_args_to_parent")
                and selector6_builder_direct_caller_context.get("entry_passes_incoming_r8_as_child_rdx")
                and selector6_builder_direct_caller_context.get("entry_passes_incoming_r9_as_child_r8")
            ),
            "selector6_builder_entry_to_builder_arg_flow_mapped": bool(
                selector6_builder_direct_caller_context.get("entry_to_builder_arg_sources", {}).get("rcx") == "entry_incoming_rcx_owner"
                and selector6_builder_direct_caller_context.get("entry_to_builder_arg_sources", {}).get("rdx") == "entry_incoming_r9"
                and selector6_builder_direct_caller_context.get("entry_to_builder_arg_sources", {}).get("r8") == "entry_incoming_r8"
            ),
            "selector6_builder_entry_thunk_has_single_helper_caller": any(
                context.get("source_va") == "0x14082a42d" and context.get("function_begin_va") == "0x14082a400"
                for context in entry_level_helper_contexts.get("selector6_builder_entry_thunk_822c70", [])
            )
            and len(entry_level_helper_contexts.get("selector6_builder_entry_thunk_822c70", [])) == 1,
            "selector6_builder_entry_helper_supplies_task_plus8_and_selector_arg": any(
                context.get("source_va") == "0x14082a42d"
                and context.get("passes_incoming_r8_to_r9")
                and context.get("passes_task_plus8_as_r8")
                and context.get("passes_stack_flag_as_edx")
                and context.get("moves_incoming_rdx_to_rbx")
                and context.get("calls_with_rbx_as_rcx")
                for context in entry_level_helper_contexts.get("selector6_builder_entry_thunk_822c70", [])
            ),
            "selector6_builder_entry_helper_vtable_slot_is_142ac7658": any(
                context.get("source_va") == "0x142ac7658"
                and context.get("vtable_base_va") == "0x142ac7648"
                and context.get("helper_matches_target")
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            ),
            "selector6_builder_entry_vtable_roles_decoded": any(
                context.get("vtable_base_va") == "0x142ac7648"
                and context.get("task_plus8_accessor_va") == "0x14082a600"
                and context.get("constructor_va") == "0x140827770"
                and context.get("copy_or_clone_va") == "0x14082b720"
                and context.get("helper_va") == "0x14082a400"
                and context.get("destructor_va") == "0x140829b20"
                and context.get("task_plus8_accessor_returns_rcx_plus8")
                and context.get("constructor_or_thunk_stores_vtable_base")
                and context.get("copy_or_clone_stores_vtable_base")
                and context.get("destructor_resets_vtable_base")
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            ),
            "selector6_builder_entry_slot0_thunk_target_mapped": any(
                context.get("vtable_base_va") == "0x142ac7648"
                and context.get("constructor_va") == "0x140827770"
                and context.get("constructor_thunk_target_va") == "0x140822680"
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            ),
            "selector6_builder_entry_alloc_clone_stores_vtable": any(
                context.get("vtable_base_va") == "0x142ac7648"
                and context.get("constructor_thunk_target_stores_vtable_base")
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            ),
            "selector6_builder_entry_alloc_clone_allocates_and_copies_plus8": any(
                context.get("vtable_base_va") == "0x142ac7648"
                and context.get("constructor_thunk_target_allocates_0xc0")
                and context.get("constructor_thunk_target_copies_task_plus8")
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            ),
            "selector6_builder_entry_alloc_clone_called_from_slot0_thunk": {
                "0x140827776"
            } == {str(ref.get("source_va")) for ref in rel32_refs.get("selector6_builder_entry_alloc_clone_822680", [])},
            "selector6_builder_entry_vtable_refs_mapped": {
                "0x1408226c8",
                "0x1408233ee",
                "0x140829b3e",
                "0x14082b750",
            }
            == {str(ref.get("source_va")) for ref in selector_entry_vtable_rip_refs.get("selector6_builder_entry_vtable_7648", [])},
            "selector6_builder_entry_vtable_refs_include_alloc_and_copy_bodies": bool(
                any(
                    ref.get("source_va") == "0x1408226c8" and ref.get("function_begin_va") == "0x140822680"
                    for ref in selector_entry_vtable_rip_refs.get("selector6_builder_entry_vtable_7648", [])
                )
                and any(
                    ref.get("source_va") == "0x1408233ee" and ref.get("function_begin_va") == "0x1408233c0"
                    for ref in selector_entry_vtable_rip_refs.get("selector6_builder_entry_vtable_7648", [])
                )
            ),
            "selector6_builder_entry_helper_distinct_from_continue_helper": any(
                context.get("vtable_base_va") == "0x142ac7648"
                for context in selector_entry_helper_vtable_contexts.get("selector6_builder_entry_helper_82a400", [])
            )
            and any(
                context.get("vtable_base_va") == "0x142ac7878"
                for context in entry_helper_vtable_contexts.get("continue_entry_helper_82a270", [])
            ),
            "selector6_builder_entry_copy_ctor_has_single_wrapper_caller": bool(
                selector6_builder_entry_owner_context.get("call_sources", {}).get("copy_ctor") == ["0x140822fb2"]
                and selector6_builder_entry_owner_context.get("copy_ctor_context", {}).get("function_begin_va") == "0x140822f60"
            ),
            "selector6_builder_entry_copy_ctor_body_copies_payload": bool(
                selector6_builder_entry_owner_context.get("copy_ctor_body_range", {}).get("begin_va") == "0x1408233c0"
                and selector6_builder_entry_owner_context.get("copy_ctor_stores_vtable_base")
                and selector6_builder_entry_owner_context.get("copy_ctor_copies_plus8_and_payload_ranges")
            ),
            "selector6_builder_entry_copy_wrapper_has_single_owner_init_caller": bool(
                selector6_builder_entry_owner_context.get("call_sources", {}).get("copy_wrapper") == ["0x140821bc4"]
                and selector6_builder_entry_owner_context.get("copy_wrapper_context", {}).get("function_begin_va") == "0x140821b70"
            ),
            "selector6_builder_entry_copy_wrapper_allocates_and_installs_owner_38": bool(
                selector6_builder_entry_owner_context.get("copy_wrapper_allocates_0xc0")
                and selector6_builder_entry_owner_context.get("copy_wrapper_passes_new_object_to_copy_ctor")
                and selector6_builder_entry_owner_context.get("copy_wrapper_installs_result_at_owner_plus_0x38")
            ),
            "selector6_builder_entry_owner_init_clears_then_builds_owner_38": bool(
                selector6_builder_entry_owner_context.get("owner_init_clears_owner_plus_0x38")
                and selector6_builder_entry_owner_context.get("owner_init_passes_stack_descriptor_and_calls_copy_wrapper")
                and selector6_builder_entry_owner_context.get("owner_init_calls_cleanup_after_copy_wrapper")
            ),
            "selector6_builder_entry_owner_init_has_single_compose_caller": bool(
                selector6_builder_entry_owner_context.get("call_sources", {}).get("owner_init") == ["0x14082887e"]
                and selector6_builder_entry_owner_context.get("owner_init_context", {}).get("function_begin_va") == "0x140828830"
            ),
            "selector6_builder_entry_owner_compose_clones_two_sources": bool(
                selector6_builder_entry_owner_context.get("owner_compose_loads_two_stack_source_objects")
                and selector6_builder_entry_owner_context.get("owner_compose_clones_two_source_plus_0x38_descriptors")
                and selector6_builder_entry_owner_context.get("owner_compose_passes_stack_clones_to_owner_init")
            ),
            "selector6_builder_entry_owner_compose_enqueues_three_stage_result": bool(
                selector6_builder_entry_owner_context.get("owner_compose_calls_task_enqueue_three_times")
            ),
            "selector6_builder_entry_owner_compose_builds_container_vtable": bool(
                selector6_builder_entry_owner_context.get("owner_compose_builds_selector_container_vtable_76b8")
            ),
            "selector6_builder_entry_owner_compose_range_mapped": bool(
                selector6_builder_entry_owner_context.get("owner_compose_body_range", {}).get("begin_va") == "0x1408288e0"
                and selector6_builder_entry_owner_context.get("owner_compose_body_range", {}).get("end_va") == "0x140828ca3"
            ),
            "selector6_builder_entry_owner_compose_has_single_parent_caller": bool(
                selector6_builder_entry_owner_context.get("call_sources", {}).get("owner_compose") == ["0x1408289a7"]
                and selector6_builder_entry_owner_context.get("owner_compose_context", {}).get("function_begin_va") is not None
            ),
            "selector6_owner_compose_has_three_parent_callers": {
                "0x140828db3",
                "0x140828f13",
                "0x14082aaa3",
            }
            == {str(context.get("source_va")) for context in selector6_owner_compose_parent_contexts},
            "selector6_owner_compose_parent_callers_have_expected_ranges": {
                "0x140828cb0",
                "0x140828e10",
                "0x14082a970",
            }
            == {str(context.get("function_begin_va")) for context in selector6_owner_compose_parent_contexts},
            "selector6_owner_compose_parent_variants_cover_local_wrapper_and_input_key": bool(
                sum(1 for context in selector6_owner_compose_parent_contexts if context.get("local_wrapper_calls")) == 2
                and sum(1 for context in selector6_owner_compose_parent_contexts if context.get("input_key_calls")) == 1
            ),
            "selector6_owner_compose_local_wrapper_variants_use_tags_7120_7130": {
                "owner_parent_local_tag_7120",
                "owner_parent_local_tag_7130",
            }.issubset(
                {
                    str(target.get("target_name"))
                    for context in selector6_owner_compose_parent_contexts
                    for target in context.get("lea_targets", [])
                }
            ),
            "selector6_owner_compose_input_variant_uses_guard_and_low_alloc": any(
                context.get("function_begin_va") == "0x14082a970"
                and context.get("input_guard_calls") == ["0x14082a9b8"]
                and context.get("low_alloc_calls") == ["0x14082a9c4"]
                for context in selector6_owner_compose_parent_contexts
            ),
            "selector6_owner_compose_parent_variants_pass_args_to_common_owner": all(
                context.get("passes_common_args_via_local_wrapper") or context.get("passes_common_args_via_input_key")
                for context in selector6_owner_compose_parent_contexts
            ),
            "selector6_owner_compose_parent_dispatch_vtables_mapped": {
                "owner_parent_dispatch_vtable_7728",
                "owner_parent_dispatch_vtable_7760",
                "owner_parent_input_vtable_76f0",
            }.issubset(
                {
                    str(target.get("target_name"))
                    for context in selector6_owner_compose_parent_contexts
                    for target in context.get("lea_targets", [])
                }
            ),
            "selector6_owner_compose_parent_callbacks_mapped": {
                "owner_parent_callback_a750",
                "owner_parent_callback_ab20",
                "owner_parent_callback_c240",
            }.issubset(
                {
                    str(target.get("target_name"))
                    for context in selector6_owner_compose_parent_contexts
                    for target in context.get("lea_targets", [])
                }
            ),
            "selector6_owner_variant_callers_are_singletons": all(
                len(selector6_owner_variant_caller_contexts.get(name, [])) == 1 for name in SELECTOR_OWNER_VARIANT_TARGET_NAMES
            ),
            "selector6_owner_variant_callers_have_expected_ranges": {
                "0x140824820",
                "0x140824a30",
                "0x140825950",
            }
            == {
                str(context.get("function_begin_va"))
                for contexts in selector6_owner_variant_caller_contexts.values()
                for context in contexts
            },
            "selector6_owner_input_variant_caller_copies_selector_payload": any(
                context.get("source_va") == "0x140824858"
                and context.get("input_variant_copies_selector_to_outparam")
                and context.get("input_variant_loads_rcx_plus_0x10_as_payload")
                and context.get("input_variant_enqueues_to_incoming_rdx")
                and context.get("enqueue_calls") == ["0x140824864"]
                for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_input_82a970", [])
            ),
            "autoload_native_selector_input_candidate_mapped": any(
                context.get("function_begin_va") == "0x14082a970"
                and context.get("input_guard_calls") == ["0x14082a9b8"]
                and context.get("input_key_calls") == ["0x14082aa7c"]
                and any(
                    target.get("target_name") == "owner_parent_input_tag_7108"
                    for target in context.get("lea_targets", [])
                )
                for context in selector6_owner_compose_parent_contexts
            ),
            "autoload_native_set_slot_callback_links_selector_to_set_save_slot": bool(
                any(
                    context.get("function_begin_va") == "0x14082a970"
                    and any(
                        target.get("target_name") == "owner_parent_callback_c240"
                        and target.get("target_va") == "0x14082c240"
                        for target in context.get("lea_targets", [])
                    )
                    for context in selector6_owner_compose_parent_contexts
                )
                and any(
                    context.get("source_va") == "0x14082c379"
                    and context.get("function_begin_va") == "0x14082c240"
                    and context.get("menu_region")
                    for context in set_save_slot_callsite_contexts
                )
            ),
            "autoload_native_playgame_submit_valid_slot_payload_mapped": bool(
                slot_reset_play_game_submit_context.get("returns_early_when_slot_minus_one")
                and slot_reset_play_game_submit_context.get("calls_selected_value_validate_then_load_pair")
                and slot_reset_play_game_submit_context.get("stores_load_pair_to_owner_job_100_104")
                and slot_reset_play_game_submit_context.get("appends_payload_vector_to_owner_job_b35f0")
            ),
            "selector6_owner_local_variant_828e10_arg_flow_mapped": any(
                context.get("source_va") == "0x140824a5b"
                and context.get("local_variant_loads_rcx_plus_8_payload")
                and context.get("local_variant_passes_incoming_r8_as_rdx")
                for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_local_828e10", [])
            ),
            "selector6_owner_local_variant_828cb0_builds_three_descriptors": any(
                context.get("source_va") == "0x140825adb"
                and context.get("large_variant_uses_three_local_descriptors")
                and {"owner_variant_continue_neighbor_7840", "owner_variant_local_7808", "owner_variant_local_7798"}.issubset(
                    {str(target.get("target_name")) for target in context.get("lea_vtables", [])}
                )
                for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_local_828cb0", [])
            ),
            "selector6_owner_local_variant_828cb0_chains_enqueue_link": any(
                context.get("source_va") == "0x140825adb"
                and context.get("large_variant_chains_enqueue_link")
                and "0x140825ab9" in context.get("enqueue_link_calls", [])
                for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_local_828cb0", [])
            ),
            "selector6_owner_local_variant_828cb0_calls_parent_with_rsi_payload": any(
                context.get("source_va") == "0x140825adb" and context.get("large_variant_calls_local_parent_828cb0")
                for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_local_828cb0", [])
            ),
            "selector6_owner_variant_callers_are_continue_adjacent": bool(
                function_ranges.get("continue_builder_caller", {}).get("begin_va") == "0x1408257e0"
                and any(
                    context.get("function_begin_va") == "0x140825950"
                    for context in selector6_owner_variant_caller_contexts.get("selector6_owner_variant_local_828cb0", [])
                )
            ),
            "continue_selector_entry_helpers_share_dispatch_shape": bool(
                continue_selector_dispatch_comparison.get("helpers_share_task_plus8_and_stack_flag_shape")
                and continue_selector_dispatch_comparison.get("continue_helper_calls_continue_entry")
                and continue_selector_dispatch_comparison.get("selector_builder_helper_calls_builder_entry")
            ),
            "selector_builder_helper_carries_extra_r8_not_continue": bool(
                continue_selector_dispatch_comparison.get("selector_helper_preserves_extra_r8_as_r9")
                and continue_selector_dispatch_comparison.get("continue_helper_has_no_extra_r8_preserve")
            ),
            "continue_selector_owner_vtable_slots_resolved": {
                "0x142ac7888": "0x14082a270",
                "0x142ac7658": "0x14082a400",
                "0x142ac71d0": "0x140826ed0",
            }
            == {
                str(row.get("slot_va")): str(row.get("value_va"))
                for row in continue_selector_dispatch_comparison.get("vtable_slot_values", [])
            },
            "selector_owner_preflight_has_vtable_slot_ref": any(
                ref.get("source_va") == "0x142ac71d0"
                for ref in continue_selector_dispatch_comparison.get("absolute_refs", {}).get("selector6_owner_preflight_slot_826ed0", [])
            ),
            "selector_owner_preflight_calls_wrapper_once": bool(
                continue_selector_dispatch_comparison.get("preflight_calls_owner_wrapper")
                and len(continue_selector_dispatch_comparison.get("owner_wrapper_caller_contexts", [])) == 1
                and continue_selector_dispatch_comparison.get("owner_wrapper_caller_contexts", [{}])[0].get("function_begin_va") == "0x140826ed0"
            ),
            "selector_owner_preflight_lazy_initialization_gate_mapped": bool(
                continue_selector_dispatch_comparison.get("preflight_checks_owner_plus_0x60")
                and any(
                    context.get("checks_owner_plus_0x60_before_wrapper")
                    for context in continue_selector_dispatch_comparison.get("owner_wrapper_caller_contexts", [])
                )
            ),
            "selector_owner_preflight_fallback_selector3_mapped": bool(
                continue_selector_dispatch_comparison.get("preflight_falls_back_to_selector_key_3")
                and any(
                    context.get("falls_back_to_selector_key_3") and context.get("fallback_calls_chain_key")
                    for context in continue_selector_dispatch_comparison.get("owner_wrapper_caller_contexts", [])
                )
            ),
            "selector_owner_preflight_success_dispatches_virtual_slot10": bool(
                continue_selector_dispatch_comparison.get("preflight_success_uses_owner_plus_0x68_virtual_slot_0x10")
                and continue_selector_dispatch_comparison.get("preflight_passes_task_plus8_float_to_virtual_path")
            ),
            "selector_owner_vtable_71c0_entry_roles_resolved": bool(
                selector_owner_lifecycle_context.get("vtables", {})
                .get("selector_owner_preflight_vtable_71c0", {})
                .get("entries", [])[0:3]
                == [
                    {"entry_va": "0x142ac71c0", "value_va": "0x140744d90", "value_rva": "0x744d90"},
                    {"entry_va": "0x142ac71c8", "value_va": "0x140826180", "value_rva": "0x826180"},
                    {"entry_va": "0x142ac71d0", "value_va": "0x140826ed0", "value_rva": "0x826ed0"},
                ]
            ),
            "selector_owner_vtable_71c0_refs_are_ctor_and_dtor": {
                "0x140821e66",
                "0x140824240",
            }
            == {
                str(ref.get("source_va"))
                for ref in selector_owner_lifecycle_context.get("vtables", {})
                .get("selector_owner_preflight_vtable_71c0", {})
                .get("rip_lea_refs", [])
            },
            "selector_owner_ctor_initializes_plus60_plus68": bool(
                selector_owner_lifecycle_context.get("constructor", {}).get("stores_vtable_71c0")
                and selector_owner_lifecycle_context.get("constructor", {}).get("allocates_0x70_bytes")
                and selector_owner_lifecycle_context.get("constructor", {}).get("copies_descriptor_to_plus_0x10")
                and selector_owner_lifecycle_context.get("constructor", {}).get("copies_payload_to_plus_0x50")
                and selector_owner_lifecycle_context.get("constructor", {}).get("clears_plus_0x60_and_plus_0x68")
            ),
            "selector_owner_dtor_releases_plus68_and_vector": bool(
                selector_owner_lifecycle_context.get("destructor", {}).get("stores_vtable_71c0")
                and selector_owner_lifecycle_context.get("destructor", {}).get("releases_plus_0x68")
                and selector_owner_lifecycle_context.get("destructor", {}).get("cleans_vector_plus_0x20")
            ),
            "selector_owner_delete_wrapper_calls_dtor_and_frees_0x70": bool(
                selector_owner_lifecycle_context.get("delete_wrapper", {}).get("calls_destructor")
                and selector_owner_lifecycle_context.get("delete_wrapper", {}).get("frees_0x70_bytes_on_delete_flag")
            ),
            "selector_owner_ctor_wrapper_calls_ctor_with_stack_payload": bool(
                selector_owner_lifecycle_context.get("constructor_wrapper", {}).get("call_sources_to_constructor") == ["0x140826482"]
                and selector_owner_lifecycle_context.get("constructor_wrapper", {}).get("passes_stack_payload_and_staging_to_ctor")
                and selector_owner_lifecycle_context.get("constructor_wrapper", {}).get("cleans_staging_vector_after_ctor")
            ),
            "selector_owner_factory_instantiates_preflight_owner_with_shared_input": bool(
                selector_owner_lifecycle_context.get("factory", {}).get("calls_main_preflight_ctor_wrapper") == ["0x1408303a7"]
                and selector_owner_lifecycle_context.get("factory", {}).get("main_call_uses_r14_plus_0x18_and_incoming_r8")
            ),
            "selector_owner_factory_chains_preflight_with_sibling_owner": bool(
                selector_owner_lifecycle_context.get("factory", {}).get("calls_sibling_ctor_wrapper") == ["0x140830319"]
                and selector_owner_lifecycle_context.get("factory", {}).get("sibling_call_uses_r14_plus_0x18_and_incoming_r8")
                and len(selector_owner_lifecycle_context.get("factory", {}).get("enqueue_link_calls", [])) >= 3
            ),
            "selector_owner_factory_thunk_chain_mapped": bool(
                selector_owner_lifecycle_context.get("factory", {}).get("caller_chain", {}).get("factory_called_from") == ["0x14082dcd7"]
                and selector_owner_lifecycle_context.get("factory", {}).get("caller_chain", {}).get("factory_thunk_called_from") == [
                    "0x14082ec3e"
                ]
                and selector_owner_lifecycle_context.get("factory", {}).get("caller_chain", {}).get("factory_outer_called_from") == [
                    "0x14082e37e"
                ]
            ),
            "selector_owner_factory_outer_caller_arg_flow_mapped": bool(
                selector_owner_factory_entry_context.get("outer_caller", {}).get("called_from") == ["0x14083709d"]
                and selector_owner_factory_entry_context.get("outer_caller", {}).get("captures_incoming_rcx_for_enqueue")
                and selector_owner_factory_entry_context.get("outer_caller", {}).get("passes_incoming_r8_as_factory_rdx")
                and selector_owner_factory_entry_context.get("outer_caller", {}).get("passes_incoming_r9_as_factory_r8")
                and selector_owner_factory_entry_context.get("outer_caller", {}).get("enqueues_factory_result_to_original_owner")
            ),
            "selector_owner_factory_entry_helper_dispatch_shape_mapped": bool(
                selector_owner_factory_entry_context.get("entry_helper", {}).get("passes_task_plus8_as_r8")
                and selector_owner_factory_entry_context.get("entry_helper", {}).get("passes_stack_flag_as_edx")
                and selector_owner_factory_entry_context.get("entry_helper", {}).get("preserves_incoming_r8_as_r9")
                and selector_owner_factory_entry_context.get("entry_helper", {}).get("calls_with_rbx_as_rcx")
                and selector_owner_factory_entry_context.get("entry_helper", {}).get("calls_outer_caller")
            ),
            "selector_owner_factory_entry_helper_has_vtable_slot": any(
                ref.get("source_va") == "0x142acb5d8"
                for ref in selector_owner_factory_entry_context.get("entry_helper", {}).get("absolute_refs", [])
            ),
            "selector_factory_entry_vtable_b5c8_entries_resolved": bool(
                selector_owner_factory_entry_context.get("vtables", {}).get("selector_factory_entry_vtable_b5c8", {}).get("entries", [])[0:3]
                == [
                    {"entry_va": "0x142acb5c8", "value_va": "0x140835380", "value_rva": "0x835380"},
                    {"entry_va": "0x142acb5d0", "value_va": "0x1408380c0", "value_rva": "0x8380c0"},
                    {"entry_va": "0x142acb5d8", "value_va": "0x140837070", "value_rva": "0x837070"},
                ]
            ),
            "selector_factory_entry_vtable_b5c8_lifecycle_refs_mapped": {
                "0x1408353a5",
                "0x1408363b2",
                "0x1408380e5",
                "0x140839517",
            }
            == {
                str(ref.get("source_va"))
                for ref in selector_owner_factory_entry_context.get("vtables", {})
                .get("selector_factory_entry_vtable_b5c8", {})
                .get("rip_lea_refs", [])
            },
            "selector_factory_entry_copy_clone_dtor_store_vtable_b5c8": bool(
                selector_owner_factory_entry_context.get("entry_copy_context", {}).get("stores_entry_vtable_b5c8")
                and selector_owner_factory_entry_context.get("entry_clone_context", {}).get("stores_entry_vtable_b5c8")
                and selector_owner_factory_entry_context.get("entry_dtor_context", {}).get("stores_entry_vtable_b5c8")
            ),
            "selector_owner_factory_path_connects_table_helper_to_preflight_factory": bool(
                selector_owner_factory_entry_context.get("entry_helper", {}).get("function_begin_va") == "0x140837070"
                and selector_owner_factory_entry_context.get("outer_caller", {}).get("function_begin_va") == "0x14082e350"
                and selector_owner_lifecycle_context.get("factory", {}).get("function_begin_va") == "0x140830210"
            ),
            "selector_factory_neighbor_vtable_bcc8_entries_resolved": bool(
                selector_owner_factory_entry_context.get("vtables", {})
                .get("selector_factory_entry_neighbor_vtable_bcc8", {})
                .get("entries", [])[0:3]
                == [
                    {"entry_va": "0x142acbcc8", "value_va": "0x140835330", "value_rva": "0x835330"},
                    {"entry_va": "0x142acbcd0", "value_va": "0x140838070", "value_rva": "0x838070"},
                    {"entry_va": "0x142acbcd8", "value_va": "0x140837020", "value_rva": "0x837020"},
                ]
            ),
            "selector_factory_neighbor_vtable_bcc8_lifecycle_refs_mapped": {
                "0x140835355",
                "0x140836372",
                "0x140838095",
                "0x1408393f0",
            }
            == {
                str(ref.get("source_va"))
                for ref in selector_owner_factory_entry_context.get("vtables", {})
                .get("selector_factory_entry_neighbor_vtable_bcc8", {})
                .get("rip_lea_refs", [])
            },
            "selector_factory_neighbor_helper_dispatch_shape_mapped": bool(
                selector_owner_factory_entry_context.get("neighbor_helper", {}).get("function_begin_va") == "0x140837020"
                and selector_owner_factory_entry_context.get("neighbor_helper", {}).get("passes_task_plus8_as_r8")
                and selector_owner_factory_entry_context.get("neighbor_helper", {}).get("passes_stack_flag_as_edx")
                and selector_owner_factory_entry_context.get("neighbor_helper", {}).get("does_not_preserve_incoming_r8_as_r9")
                and selector_owner_factory_entry_context.get("neighbor_helper", {}).get("calls_with_rbx_as_rcx")
            ),
            "selector_factory_neighbor_helper_has_vtable_slot": any(
                ref.get("source_va") == "0x142acbcd8"
                for ref in selector_owner_factory_entry_context.get("neighbor_helper", {}).get("absolute_refs", [])
            ),
            "selector_factory_neighbor_outer_chain_mapped": bool(
                selector_owner_factory_entry_context.get("neighbor_outer", {}).get("function_begin_va") == "0x14082e310"
                and selector_owner_factory_entry_context.get("neighbor_outer", {}).get("called_from") == ["0x14083704a"]
                and selector_owner_factory_entry_context.get("neighbor_outer", {}).get("passes_incoming_r8_as_rdx")
                and selector_owner_factory_entry_context.get("neighbor_outer", {}).get("does_not_pass_incoming_r9")
                and selector_owner_factory_entry_context.get("neighbor_chain", {}).get("outer_calls_thunk") == ["0x14082e331"]
                and selector_owner_factory_entry_context.get("neighbor_chain", {}).get("thunk_calls_factory") == ["0x14082ebfe"]
                and selector_owner_factory_entry_context.get("neighbor_chain", {}).get("factory_called_from_thunk") == ["0x14082dc97"]
            ),
            "selector_factory_complex_builder_8394b0_mapped": bool(
                selector_owner_factory_entry_context.get("complex_builder_context", {}).get("function_begin_va") == "0x1408394b0"
                and selector_owner_factory_entry_context.get("complex_builder_context", {}).get("stores_entry_vtable_b5c8")
                and selector_owner_factory_entry_context.get("complex_builder_context", {}).get("copies_two_qwords_from_rcx")
                and selector_owner_factory_entry_context.get("complex_builder_context", {}).get("submits_with_selector_0xa")
            ),
            "selector_factory_complex_builder_calls_submit_834b40": any(
                ref.get("source_va") == "0x140839548"
                for ref in selector_owner_factory_entry_context.get("complex_builder_context", {}).get("calls_complex_submit", [])
            ),
            "selector_submit_helper_range_mapped": bool(
                selector_submit_context.get("function_begin_va") == "0x140834b40"
                and selector_submit_context.get("function_end_va") == "0x140834d44"
            ),
            "selector_submit_helper_has_six_known_callers": {
                "0x140839548",
                "0x14083985e",
                "0x140839946",
                "0x14083a203",
                "0x14083a29e",
                "0x14083a513",
            }
            == set(selector_submit_context.get("caller_sources", [])),
            "selector_submit_helper_captures_args_and_stack_context": bool(
                selector_submit_context.get("captures_incoming_rcx_rdx_r8_r9")
                and selector_submit_context.get("reads_stack_arg_0x130_into_rbx")
                and selector_submit_context.get("stores_selector_argument_r8d")
            ),
            "selector_submit_helper_reads_owner_plus38_and_virtual_builder": bool(
                selector_submit_context.get("reads_owner_plus_0x38_source")
                and selector_submit_context.get("calls_owner_plus0_virtual_builder")
            ),
            "selector_submit_helper_builds_descriptor_vtable_bde0": bool(
                selector_submit_context.get("builds_descriptor_vtable_bde0")
                and selector_submit_context.get("copies_incoming_payload_into_descriptor")
            ),
            "selector_submit_helper_calls_builder_clone_final_enqueue": bool(
                selector_submit_context.get("called_targets", {}).get("selector_submit_descriptor_builder_82d840") == ["0x140834c6e"]
                and selector_submit_context.get("called_targets", {}).get("selector_submit_clone_wrapper_8347a0") == ["0x140834c7e"]
                and selector_submit_context.get("called_targets", {}).get("selector_submit_final_enqueue_7917e0") == ["0x140834c8f"]
            ),
            "selector_submit_helper_final_enqueue_preserves_owner_and_descriptor": bool(
                selector_submit_context.get("passes_descriptor_builder_into_clone_wrapper")
                and selector_submit_context.get("final_enqueue_uses_original_rcx_owner_and_descriptor")
            ),
            "selector_submit_helper_cleans_temporaries_and_returns_owner": bool(
                selector_submit_context.get("cleans_temporary_owned_objects") and selector_submit_context.get("returns_original_owner_rdi")
            ),
            "selector_final_enqueue_helper_range_mapped": bool(
                selector_final_enqueue_context.get("function_begin_va") == "0x1407917e0"
                and selector_final_enqueue_context.get("function_end_va") == "0x1407918fa"
            ),
            "selector_final_enqueue_helper_has_single_submit_caller": selector_final_enqueue_context.get("caller_sources") == [
                "0x140834c8f"
            ],
            "selector_final_enqueue_helper_captures_args_and_allocates_pair": bool(
                selector_final_enqueue_context.get("captures_incoming_rcx_rdx_r8")
                and selector_final_enqueue_context.get("allocates_0xa0_pair_object")
            ),
            "selector_final_enqueue_helper_copies_both_sources": bool(
                selector_final_enqueue_context.get("copies_r8_plus38_source_into_stack48")
                and selector_final_enqueue_context.get("copies_rcx_plus38_source_into_stack88")
            ),
            "selector_final_enqueue_helper_calls_pair_builder": bool(
                selector_final_enqueue_context.get("calls_pair_builder_with_stack_descriptors")
            ),
            "selector_final_enqueue_helper_stores_result_and_refs": bool(
                selector_final_enqueue_context.get("stores_pair_result_to_output_rdx")
                and selector_final_enqueue_context.get("increments_ref_if_result_nonnull")
            ),
            "selector_final_enqueue_helper_cleans_r8_source_and_returns_output": bool(
                selector_final_enqueue_context.get("cleans_r8_plus38_after_pair_build")
                and selector_final_enqueue_context.get("returns_output_pointer_rdi")
            ),
            "selector_final_enqueue_callers_have_expected_ranges": {
                "0x14080da8b": "0x14080d990",
                "0x140827900": "0x1408277d0",
                "0x14082ad64": "0x14082ac70",
                "0x140834c8f": "0x140834b40",
                "0x1409ac6fc": "0x1409ac620",
            }
            == {
                str(context.get("source_va")): str(context.get("function_begin_va"))
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            },
            "selector_final_enqueue_selector_submit_callsite_identified": any(
                context.get("is_selector_submit_callsite")
                and (
                    context.get("call_window_uses_descriptor_output_rdx")
                    or context.get("call_window_uses_saved_output_rdi_as_rdx")
                )
                and context.get("call_window_uses_stack_descriptor_r8")
                and context.get("call_window_uses_rax_as_rcx")
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            ),
            "selector_final_enqueue_entry_family_callsite_identified": any(
                context.get("is_entry_family_callsite") and context.get("menu_region")
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            ),
            "selector_final_enqueue_selector_builder_callsite_identified": any(
                context.get("is_selector_builder_callsite") and context.get("menu_region")
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            ),
            "selector_final_enqueue_global_runtime_callsite_identified": any(
                context.get("is_global_runtime_callsite") and not context.get("menu_region")
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            ),
            "selector_final_enqueue_outside_menu_callsite_identified": any(
                context.get("is_outside_menu_runtime_callsite") and not context.get("menu_region")
                for context in selector_final_enqueue_context.get("caller_contexts", [])
            ),
            "selector_final_enqueue_runtime_trace_filter_subset_identified": {
                "0x140827900",
                "0x14082ad64",
                "0x140834c8f",
            }.issubset(
                {
                    str(context.get("source_va"))
                    for context in selector_final_enqueue_context.get("caller_contexts", [])
                    if context.get("menu_region")
                }
            ),
            "selector_pair_builder_range_mapped": bool(
                selector_pair_builder_context.get("function_begin_va") == "0x140790fa0"
                and selector_pair_builder_context.get("function_end_va") == "0x14079106a"
            ),
            "selector_pair_builder_has_single_final_enqueue_caller": selector_pair_builder_context.get("caller_sources") == [
                "0x1407918ab"
            ],
            "selector_pair_builder_captures_output_and_sources": bool(
                selector_pair_builder_context.get("captures_output_rcx_and_sources_rdx_r8")
            ),
            "selector_pair_builder_initializes_pair_vtable_and_slots": bool(
                selector_pair_builder_context.get("calls_header_init_before_vtable_store")
                and selector_pair_builder_context.get("stores_pair_vtable_aa26f0")
                and selector_pair_builder_context.get("clears_output_slots_10_18")
            ),
            "selector_pair_builder_clones_both_source_descriptors": bool(
                selector_pair_builder_context.get("clones_left_source_plus38_to_output_plus20")
                and selector_pair_builder_context.get("clones_right_source_plus38_to_output_plus60")
            ),
            "selector_pair_builder_cleans_sources_and_returns_output": bool(
                selector_pair_builder_context.get("cleans_left_source_after_clone")
                and selector_pair_builder_context.get("cleans_right_source_after_clone")
                and selector_pair_builder_context.get("returns_output_rbx")
            ),
            "set_save_slot_has_five_known_callers": {
                "0x14058d456",
                "0x1406098ca",
                "0x140789110",
                "0x14082c379",
                "0x140b0cd8b",
            }
            == {str(context.get("source_va")) for context in set_save_slot_callsite_contexts},
            "set_save_slot_callers_have_expected_ranges": {
                "0x14058d456": "0x14058d3c0",
                "0x1406098ca": "0x140609820",
                "0x140789110": "0x1407890ec",
                "0x14082c379": "0x14082c240",
                "0x140b0cd8b": "0x140b0cd70",
            }
            == {
                str(context.get("source_va")): str(context.get("function_begin_va"))
                for context in set_save_slot_callsite_contexts
            },
            "set_save_slot_menu_wrapper_callsite_mapped": any(
                context.get("source_va") == "0x14082c379"
                and context.get("function_begin_va") == "0x14082c240"
                and context.get("menu_region")
                for context in set_save_slot_callsite_contexts
            ),
            "set_save_slot_runtime_reset_callsite_mapped": any(
                context.get("source_va") == "0x140b0cd8b"
                and context.get("return_va") == "0x140b0cd90"
                and context.get("function_begin_va") == "0x140b0cd70"
                and context.get("passes_minus_one_in_ecx")
                for context in set_save_slot_callsite_contexts
            ),
            "set_save_slot_runtime_reset_has_counter_gate_and_cleanup": any(
                context.get("source_va") == "0x140b0cd8b"
                and context.get("increments_owner_plus_b0_before_call")
                and context.get("checks_owner_plus_b0_before_call")
                and context.get("stores_owner_plus_4c_minus_one_after_call")
                for context in set_save_slot_callsite_contexts
            ),
            "slot_reset_parent_loop_range_mapped": bool(
                slot_reset_dispatch_context.get("parent_function_begin_va") == "0x140b0bd60"
                and slot_reset_dispatch_context.get("parent_function_end_va") == "0x140b0bfa3"
            ),
            "slot_reset_parent_entry_thunks_mapped": {"0x140b0bd40", "0x140b0bd50"}.issubset(
                set(slot_reset_dispatch_context.get("parent_ref_sources", []))
            ),
            "slot_reset_parent_state_table_dispatch_mapped": bool(
                slot_reset_dispatch_context.get("parent_captures_owner_rbx_and_arg_rsi")
                and slot_reset_dispatch_context.get("parent_copies_state_4c_to_current_48")
                and slot_reset_dispatch_context.get("parent_updates_label_a0_from_table_plus8")
                and slot_reset_dispatch_context.get("parent_dispatches_state_table_entry")
                and slot_reset_dispatch_context.get("parent_refreshes_state_after_dispatch")
            ),
            "slot_reset_parent_default_label_and_loop_guard_mapped": bool(
                slot_reset_dispatch_context.get("parent_sets_default_label_for_minus_one_state")
                and slot_reset_dispatch_context.get("parent_calls_owner_virtual_slot20_before_dispatch")
                and slot_reset_dispatch_context.get("parent_loop_flag_and_cap_mapped")
            ),
            "slot_reset_parent_loop_trace_state_payload_mapped": bool(
                slot_reset_dispatch_context.get("parent_trace_maps_state_table_requested_current_label")
                and slot_reset_dispatch_context.get("parent_trace_dispatch_callsite_mapped")
            ),
            "slot_reset_parent_loop_trace_control_payload_mapped": bool(
                slot_reset_dispatch_context.get("parent_trace_maps_loop_flags_and_counters")
            ),
            "slot_reset_parent_loop_trace_queue_payload_mapped": bool(
                slot_reset_title_queue_producer_context.get("seed_resets_owner130_selection_and_advances")
                and slot_reset_title_queue_producer_context.get("pump_takes_owner128_queue_after_selection")
                and slot_reset_menu_job_wait_context.get("queues_owner_130_timed_task")
            ),
            "slot_reset_parent_loop_trace_finish_payload_mapped": bool(
                slot_reset_dispatch_context.get("handler_increments_owner_b0_and_requires_gt_one")
                and slot_reset_dispatch_context.get("handler_marks_owner_4c_minus_one_after_reset")
            ),
            "slot_reset_parent_loop_trace_payload_ready_for_passive_probe": bool(
                slot_reset_title_parent_ctor_context.get("passive_trace_anchor_is_parent_loop_not_simple_table")
                and slot_reset_dispatch_context.get("parent_trace_maps_state_table_requested_current_label")
                and slot_reset_dispatch_context.get("parent_trace_maps_loop_flags_and_counters")
                and slot_reset_title_queue_producer_context.get("pump_takes_owner128_queue_after_selection")
                and slot_reset_menu_job_wait_context.get("queues_owner_130_timed_task")
                and slot_reset_dispatch_context.get("handler_increments_owner_b0_and_requires_gt_one")
            ),
            "slot_reset_to_menu_job_wait_helper_range_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("helper_begin_va") == "0x140b0e530"
                and slot_reset_to_menu_job_wait_context.get("helper_end_va") == "0x140b0e650"
            ),
            "slot_reset_to_menu_job_wait_helper_attaches_owner130": bool(
                slot_reset_to_menu_job_wait_context.get("attaches_payload_to_owner130")
            ),
            "slot_reset_to_menu_job_wait_helper_sets_state10": bool(
                slot_reset_to_menu_job_wait_context.get("sets_state10_menujobwait")
            ),
            "slot_reset_to_menu_job_wait_helper_callers_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("callers_are_title_bootstrap_states")
            ),
            "slot_reset_to_menu_job_wait_helper_candidate_handoff_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("candidate_title_to_menujobwait_handoff_mapped")
                and slot_reset_to_menu_job_wait_context.get("releases_payload_after_transition")
            ),
            "slot_reset_to_menu_job_wait_begin_logo_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begin_logo_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_begin_title_accept_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begin_title_accept_payload_context_mapped")
            ),
            "slot_reset_begintitle_bool_wrapper_inner_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_bool_wrapper_maps_final_wrapper_inner")
            ),
            "slot_reset_begintitle_owner_builder_range_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_range_mapped")
            ),
            "slot_reset_begintitle_owner_builder_args_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_args_owner_e0_138")
            ),
            "slot_reset_begintitle_owner_builder_input_call_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_calls_input_condition_at_fa8b")
            ),
            "slot_reset_begintitle_owner_builder_selector6_active_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_selector6_active")
            ),
            "slot_reset_begintitle_input_builder_allocates_0x138": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_condition_builder_allocates_0x138")
            ),
            "slot_reset_begintitle_owner_builder_sources_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_passes_descriptor_to_input_condition")
                and slot_reset_to_menu_job_wait_context.get("begintitle_owner_builder_preserves_owner_sources")
            ),
            "slot_reset_begintitle_input_condition_builder_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_condition_builder_selector6_active")
                and slot_reset_to_menu_job_wait_context.get("begintitle_input_condition_builder_owner_sources")
            ),
            "slot_reset_begintitle_input_builder_range_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_range_mapped")
            ),
            "slot_reset_begintitle_input_builder_arg_capture_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_captures_all_args")
            ),
            "slot_reset_begintitle_input_builder_alloc_ctor_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_allocates_and_constructs_0x138_node")
            ),
            "slot_reset_begintitle_input_builder_temp_descriptor_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_temp_descriptor_vtables_mapped")
            ),
            "slot_reset_begintitle_input_builder_node_vtable_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_node_vtable_mapped")
            ),
            "slot_reset_begintitle_input_builder_copy_sources_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_copies_selector_and_sources")
            ),
            "slot_reset_begintitle_input_builder_subnodes_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_initializes_condition_subnodes")
            ),
            "slot_reset_begintitle_input_builder_output_cleanup_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_returns_output_and_cleans_sources")
            ),
            "slot_reset_begintitle_input_node_vtable_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_vtable_update_slot_mapped")
            ),
            "slot_reset_begintitle_input_node_update_range_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_range_mapped")
            ),
            "slot_reset_begintitle_input_node_update_gates_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_gate_fields_mapped")
            ),
            "slot_reset_begintitle_input_node_update_child_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_moves_child_to_130")
                and slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_child_submit_mapped")
            ),
            "slot_reset_begintitle_input_node_update_status_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_status_switch_mapped")
            ),
            "slot_reset_begintitle_input_node_update_global_bit_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_global_input_bit_mapped")
            ),
            "slot_reset_begintitle_input_node_update_terminal_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_terminal_descriptor_mapped")
            ),
            "slot_reset_begintitle_input_node_update_wait_states_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_node_update_wait_states_mapped")
            ),
            "slot_reset_begintitle_input_builder_subnode_update_offsets_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_builder_subnode_offsets_match_update")
            ),
            "slot_reset_begintitle_temp_vtable_slots_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_temp_vtable_slots_mapped")
            ),
            "slot_reset_begintitle_temp_clone_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_temp_clone_copies_callback_descriptor")
                and slot_reset_to_menu_job_wait_context.get("begintitle_temp_active_clone_copies_callback_descriptor")
            ),
            "slot_reset_begintitle_temp_child_clone_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_temp_child_clone_wraps_source40")
            ),
            "slot_reset_begintitle_temp_callback_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_temp_callback_inverts_input_manager_state")
            ),
            "slot_reset_begintitle_input_manager_state_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_state_active18_mapped")
            ),
            "slot_reset_begintitle_input_manager_global_ref_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_global_callback_ref_mapped")
            ),
            "slot_reset_begintitle_input_manager_singleton_refs_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_singleton_lifecycle_refs_mapped")
            ),
            "slot_reset_begintitle_input_manager_shutdown_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_shutdown_clears_singleton")
            ),
            "slot_reset_begintitle_input_manager_init_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_init_allocates_singleton")
                and slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_init_updates_subsystems")
            ),
            "slot_reset_begintitle_input_manager_counter_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_init_captures_frame_counter")
            ),
            "slot_reset_begintitle_input_manager_queue_setup_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_input_manager_queue_setup_allocates_child80")
            ),
            "slot_reset_begintitle_branch_step_wrapper_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_branch_step_wrapper_maps_builder_and_attach")
            ),
            "slot_reset_begintitle_accept_condition_chain_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("begintitle_accept_condition_chain_mapped")
            ),
            "slot_reset_to_menu_job_wait_xr_dialog_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("xr_dialog_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_init_menu_two_descriptor_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("init_menu_two_descriptor_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_init_menu_gate_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("init_menu_gate_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_init_menu_link_fold_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("init_menu_link_fold_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_init_menu_payload_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("init_menu_payload_context_mapped")
            ),
            "slot_reset_to_menu_job_wait_all_caller_payloads_mapped": bool(
                slot_reset_to_menu_job_wait_context.get("all_handoff_callers_payload_context_mapped")
            ),
            "slot_reset_title_parent_constructor_ranges_mapped": bool(
                slot_reset_title_parent_ctor_context.get("wrapper_begin_va") == "0x140b0b020"
                and slot_reset_title_parent_ctor_context.get("wrapper_end_va") == "0x140b0b0c6"
                and slot_reset_title_parent_ctor_context.get("base_ctor_begin_va") == "0x140b0b0d0"
                and slot_reset_title_parent_ctor_context.get("base_ctor_end_va") == "0x140b0b1b2"
                and slot_reset_title_parent_ctor_context.get("title_ctor_begin_va") == "0x140b0b1c0"
                and slot_reset_title_parent_ctor_context.get("title_ctor_end_va") == "0x140b0b2f0"
            ),
            "slot_reset_title_parent_constructor_table_chain_mapped": bool(
                slot_reset_title_parent_ctor_context.get("title_ctor_passes_title_table_to_wrapper")
                and slot_reset_title_parent_ctor_context.get("wrapper_captures_table_arg_and_forwards_as_r8")
                and slot_reset_title_parent_ctor_context.get("base_ctor_stores_table_pointer_to_owner10")
            ),
            "slot_reset_title_parent_constructor_state_fields_mapped": bool(
                slot_reset_title_parent_ctor_context.get("base_ctor_initializes_state_fields")
                and slot_reset_title_parent_ctor_context.get("title_ctor_sets_final_vtable_and_state0")
                and slot_reset_title_parent_ctor_context.get("title_ctor_initializes_owner_128_130")
            ),
            "slot_reset_title_parent_constructor_to_dispatch_mapped": bool(
                slot_reset_title_parent_ctor_context.get("constructor_chain_to_parent_table_dispatch_mapped")
            ),
            "slot_reset_title_parent_title_table_refs_mapped": bool(
                slot_reset_title_parent_ctor_context.get("title_table_refs_include_initializer_and_constructor")
            ),
            "slot_reset_title_parent_simple_table_refs_initializer_only": bool(
                slot_reset_title_parent_ctor_context.get("simple_table_refs_are_initializer_only")
            ),
            "slot_reset_title_parent_final_vtable_slot28_maps_parent_loop": bool(
                slot_reset_title_parent_ctor_context.get("title_ctor_final_vtable_parent_loop_slot28")
            ),
            "slot_reset_title_parent_passive_trace_anchor_mapped": bool(
                slot_reset_title_parent_ctor_context.get("passive_trace_anchor_is_parent_loop_not_simple_table")
            ),
            "slot_reset_handler_range_mapped": bool(
                slot_reset_dispatch_context.get("handler_function_begin_va") == "0x140b0cd70"
                and slot_reset_dispatch_context.get("handler_function_end_va") == "0x140b0cdde"
            ),
            "slot_reset_handler_counter_gate_and_minus_one_slot_mapped": bool(
                slot_reset_dispatch_context.get("handler_increments_owner_b0_and_requires_gt_one")
                and slot_reset_dispatch_context.get("handler_passes_minus_one_to_set_save_slot")
            ),
            "slot_reset_handler_global_notify_and_state_cleanup_mapped": bool(
                slot_reset_dispatch_context.get("handler_uses_global_after_reset")
                and slot_reset_dispatch_context.get("handler_marks_owner_4c_minus_one_after_reset")
            ),
            "slot_reset_parent_vtable_refs_mapped": bool(
                len(slot_reset_dispatch_context.get("parent_vtable_contexts", [])) == 4
                and all(
                    context.get("section") == ".rdata"
                    for context in slot_reset_dispatch_context.get("parent_vtable_contexts", [])
                )
            ),
            "slot_reset_parent_vtable_slot28_is_loop": all(
                any(
                    entry.get("entry_va") == context.get("source_va")
                    and entry.get("value_va") == "0x140b0bd60"
                    for entry in context.get("entries", [])
                )
                for context in slot_reset_dispatch_context.get("parent_vtable_contexts", [])
            ),
            "slot_reset_parent_vtable_entry20_variants_mapped": {"0x140126730", "0x140b0bd40", "0x140b0bd50"}.issubset(
                {
                    str(entry.get("value_va"))
                    for context in slot_reset_dispatch_context.get("parent_vtable_contexts", [])
                    for entry in context.get("entries", [])
                    if int(str(entry.get("entry_va")), 16) - int(str(context.get("vtable_base_va")), 16) == 0x20
                }
            ),
            "slot_reset_parent_vtable_dispatch_tail_shared": all(
                {
                    "0x140b0c240",
                    "0x140b0c0f0",
                    "0x140b0b9a0",
                    "0x140b0b9c0",
                }.issubset({str(entry.get("value_va")) for entry in context.get("entries", [])})
                for context in slot_reset_dispatch_context.get("parent_vtable_contexts", [])
            ),
            "slot_reset_state_table_init_range_mapped": bool(
                slot_reset_state_table_init_context.get("function_begin_va") == "0x1400a4f50"
                and slot_reset_state_table_init_context.get("function_end_va") == "0x1400a50bd"
            ),
            "slot_reset_state_table_zeroes_global_0xd0": bool(
                slot_reset_state_table_init_context.get("table_base_va") == "0x143d71580"
                and slot_reset_state_table_init_context.get("zeroes_table_base")
                and slot_reset_state_table_init_context.get("zero_size_bytes") == 0xD0
            ),
            "slot_reset_state_table_has_12_initialized_pairs": bool(
                len(slot_reset_state_table_init_context.get("entries", [])) == 12
                and all(entry.get("handler_va") and entry.get("label_va") for entry in slot_reset_state_table_init_context.get("entries", []))
            ),
            "slot_reset_state_table_handler_index_11_is_finish_reset": any(
                entry.get("entry_index") == 11
                and entry.get("handler_va") == "0x140b0cd70"
                and entry.get("label_text") == "TitleStep::STEP_Finish"
                for entry in slot_reset_state_table_init_context.get("entries", [])
            ),
            "slot_reset_state_table_neighbor_handlers_mapped": {
                6: "0x140b0cde0",
                7: "0x140b0cc00",
                8: "0x140b0ccc0",
                9: "0x140b0d550",
                10: "0x140b0d400",
                11: "0x140b0cd70",
            }.items()
            <= {
                int(entry.get("entry_index")): str(entry.get("handler_va"))
                for entry in slot_reset_state_table_init_context.get("entries", [])
            }.items(),
            "slot_reset_menu_job_wait_range_mapped": bool(
                slot_reset_menu_job_wait_context.get("menu_job_wait_begin_va") == "0x140b0d400"
                and slot_reset_menu_job_wait_context.get("menu_job_wait_end_va") == "0x140b0d549"
            ),
            "slot_reset_menu_job_wait_builds_owner_tasks": bool(
                slot_reset_menu_job_wait_context.get("captures_owner_rbx_and_task_arg_rdi")
                and slot_reset_menu_job_wait_context.get("sets_global_job_active_flag_6b0")
                and slot_reset_menu_job_wait_context.get("builds_timed_descriptor_from_arg_plus8")
                and slot_reset_menu_job_wait_context.get("submits_owner_e0_timed_task")
                and slot_reset_menu_job_wait_context.get("queues_owner_130_timed_task")
            ),
            "slot_reset_menu_job_wait_global_toggle_mapped": bool(
                slot_reset_menu_job_wait_context.get("calls_global_toggle_after_first_submit")
            ),
            "slot_reset_global_toggle_helper_store_mapped": bool(
                slot_reset_global_toggle_context.get("helper_va") == "0x1407663c0"
                and slot_reset_global_toggle_context.get("stores_dl_to_context_plus_18")
            ),
            "slot_reset_global_toggle_callers_mapped": bool(
                slot_reset_global_toggle_context.get("ending_menujobwait_call") == "0x140ae5432"
                and slot_reset_global_toggle_context.get("title_menujobwait_call") == "0x140b0d4e9"
            ),
            "slot_reset_global_toggle_callers_pass_true_after_first_submit": bool(
                slot_reset_global_toggle_context.get("caller_windows", {})
                .get("0x140ae5432", {})
                .get("passes_true_in_dl")
                and slot_reset_global_toggle_context.get("caller_windows", {})
                .get("0x140b0d4e9", {})
                .get("passes_true_in_dl")
            ),
            "slot_reset_global_toggle_extra_parent_range_mapped": bool(
                slot_reset_global_toggle_context.get("extra_parent_begin_va") == "0x140b01be0"
                and slot_reset_global_toggle_context.get("extra_parent_end_va") == "0x140b0210e"
                and slot_reset_global_toggle_context.get("extra_parent_callers") == ["0x140b01daf", "0x140b01f90"]
            ),
            "slot_reset_global_toggle_extra_parent_first_call_can_clear_or_set": bool(
                slot_reset_global_toggle_context.get("extra_parent_first_call_has_false_and_true_paths")
            ),
            "slot_reset_global_toggle_extra_parent_second_call_sets_19_and_true": bool(
                slot_reset_global_toggle_context.get("extra_parent_second_call_sets_context_plus19_then_true")
            ),
            "slot_reset_global_toggle_extra_parent_clears_19_and_gates_first_toggle": bool(
                slot_reset_global_toggle_context.get("extra_parent_clears_context_plus19_before_first_toggle")
                and slot_reset_global_toggle_context.get("extra_parent_first_toggle_follows_a9cc90_gate")
            ),
            "slot_reset_global_toggle_extra_parent_second_toggle_follows_failed_gate": bool(
                slot_reset_global_toggle_context.get("extra_parent_second_toggle_follows_failed_a9cd00_gate")
            ),
            "slot_reset_global_toggle_extra_parent_callback_chains_mapped": bool(
                slot_reset_global_toggle_context.get("extra_parent_first_toggle_callback_chain_mapped")
                and slot_reset_global_toggle_context.get("extra_parent_second_toggle_callback_chain_mapped")
            ),
            "slot_reset_global_job_context_known_functions_mapped": bool(
                {"0x140ae5390", "0x140b01be0", "0x140b0d400"}.issubset(
                    set(slot_reset_global_toggle_context.get("global_context_functions", {}).keys())
                )
            ),
            "slot_reset_global_job_context_parent_writes_plus19": bool(
                slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x140b01be0", {})
                .get("writes_plus19_direct")
            ),
            "slot_reset_global_job_context_menujobwait_refs_without_direct_plus19": bool(
                slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x140ae5390", {})
                .get("writes_plus19_direct")
                is False
                and slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x140b0d400", {})
                .get("writes_plus19_direct")
                is False
            ),
            "slot_reset_global_context_plus19_readers_identified": bool(
                slot_reset_global_toggle_context.get("global_context_plus19_reader_functions")
                == ["0x14075811b", "0x140a832a0"]
            ),
            "slot_reset_global_context_plus19_reader_75811b_gate_mapped": bool(
                slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x14075811b", {})
                .get("plus19_cmp_count")
                == 1
                and slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x14075811b", {})
                .get("ref_sources")
                == ["0x14075814e"]
            ),
            "slot_reset_global_context_plus19_reader_a832a0_list_mapped": bool(
                slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x140a832a0", {})
                .get("plus19_cmp_count", 0)
                >= 12
                and slot_reset_global_toggle_context.get("global_context_functions", {})
                .get("0x140a832a0", {})
                .get("node_plus18_mark_count", 0)
                >= 3
            ),
            "slot_reset_global_context_plus19_gate_range_mapped": bool(
                slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "function_begin_va"
                )
                == "0x140758050"
                and slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "function_end_va"
                )
                == "0x140758199"
            ),
            "slot_reset_global_context_plus19_gate_callers_mapped": bool(
                slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get("caller_count")
                == 19
            ),
            "slot_reset_global_context_plus19_gate_conditions_mapped": bool(
                slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "loads_global_context_before_plus1a_check"
                )
                and slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "queries_status_when_plus1a_differs"
                )
                and slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "requires_input_byte_before_plus19_gate"
                )
                and slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "requires_context_798_empty_and_plus19_true"
                )
                and slot_reset_global_toggle_context.get("global_context_plus19_gate_context", {}).get(
                    "also_requires_status_result_nonzero"
                )
            ),
            "slot_reset_title_queue_state_table_range_mapped": bool(
                slot_reset_title_queue_state_table_context.get("function_begin_va") == "0x1400a4c90"
                and slot_reset_title_queue_state_table_context.get("function_end_va") == "0x1400a4d53"
            ),
            "slot_reset_title_queue_state_table_zeroes_global_0x60": bool(
                slot_reset_title_queue_state_table_context.get("table_base_va") == "0x143d71340"
                and slot_reset_title_queue_state_table_context.get("zeroes_table_base")
                and slot_reset_title_queue_state_table_context.get("zero_size_bytes") == 0x60
            ),
            "slot_reset_title_queue_state_table_has_6_initialized_pairs": bool(
                len(slot_reset_title_queue_state_table_context.get("entries", [])) == 6
                and all(
                    entry.get("handler_va") and entry.get("label_va")
                    for entry in slot_reset_title_queue_state_table_context.get("entries", [])
                )
            ),
            "slot_reset_title_queue_menu_states_link_seed_and_pump": bool(
                [
                    (entry.get("entry_index"), entry.get("handler_va"), entry.get("label_text"))
                    for entry in slot_reset_title_queue_state_table_context.get("entries", [])[:2]
                ]
                == [
                    (0, "0x140b0a4a0", "SimpleTitleStep::STEP_MenuInit"),
                    (1, "0x140b0a5e0", "SimpleTitleStep::STEP_MenuLoop"),
                ]
            ),
            "slot_reset_title_queue_ingame_states_mapped": bool(
                [
                    (entry.get("entry_index"), entry.get("handler_va"), entry.get("label_text"))
                    for entry in slot_reset_title_queue_state_table_context.get("entries", [])[2:]
                ]
                == [
                    (2, "0x140b0a1f0", "SimpleTitleStep::STEP_IngameInit"),
                    (3, "0x140b0a430", "SimpleTitleStep::STEP_IngameStandby"),
                    (4, "0x140b0a1b0", "SimpleTitleStep::STEP_Ingame"),
                    (5, "0x140b0a170", "SimpleTitleStep::STEP_End"),
                ]
            ),
            "slot_reset_title_queue_state_handler_ranges_mapped": bool(
                slot_reset_title_queue_state_table_context.get("handler_ranges", {}).get("0x140b0a4a0", {}).get("begin_va")
                == "0x140b0a4a0"
                and slot_reset_title_queue_state_table_context.get("handler_ranges", {}).get("0x140b0a5e0", {}).get("begin_va")
                == "0x140b0a5e0"
                and slot_reset_title_queue_state_table_context.get("handler_ranges", {}).get("0x140b0a1f0", {}).get("begin_va")
                == "0x140b0a1f0"
                and slot_reset_title_queue_state_table_context.get("handler_ranges", {}).get("0x140b0a170", {}).get("end_va")
                == "0x140b0a1ab"
            ),
            "slot_reset_title_queue_seed_range_mapped": bool(
                slot_reset_title_queue_producer_context.get("seed_begin_va") == "0x140b0a4a0"
                and slot_reset_title_queue_producer_context.get("seed_end_va") == "0x140b0a5db"
            ),
            "slot_reset_title_queue_seed_produces_owner128_task": bool(
                slot_reset_title_queue_producer_context.get("seed_builds_task_from_owner_d8")
                and slot_reset_title_queue_producer_context.get("seed_enqueues_task_and_attaches_to_owner128")
            ),
            "slot_reset_title_queue_seed_resets_selection_and_advances": bool(
                slot_reset_title_queue_producer_context.get("seed_resets_owner130_selection_and_advances")
            ),
            "slot_reset_title_queue_pump_range_mapped": bool(
                slot_reset_title_queue_producer_context.get("pump_begin_va") == "0x140b0a5e0"
                and slot_reset_title_queue_producer_context.get("pump_end_va") == "0x140b0a97f"
            ),
            "slot_reset_title_queue_pump_consumes_owner128_task": bool(
                slot_reset_title_queue_producer_context.get("pump_submits_owner_d8_and_queues_owner128")
                and slot_reset_title_queue_producer_context.get("pump_advances_when_owner128_empty")
            ),
            "slot_reset_title_queue_pump_selection_path_mapped": bool(
                slot_reset_title_queue_producer_context.get("pump_parses_stream_selection_to_owner130")
                and slot_reset_title_queue_producer_context.get("pump_takes_owner128_queue_after_selection")
                and slot_reset_title_queue_producer_context.get("pump_advances_after_selection_queue_take")
            ),
            "slot_reset_title_queue_advance_helper_mapped": bool(
                slot_reset_title_queue_producer_context.get("advance_begin_va") == "0x140b0a980"
                and slot_reset_title_queue_producer_context.get("advance_end_va") == "0x140b0aa89"
                and slot_reset_title_queue_producer_context.get("advance_increments_owner_4c_and_bounds_owner_48")
            ),
            "slot_reset_title_queue_advance_callers_mapped": bool(
                slot_reset_title_queue_producer_context.get("advance_callers_cover_seed_pump_and_guard_paths")
            ),
            "slot_reset_title_queue_set_state_helper_mapped": bool(
                slot_reset_title_queue_producer_context.get("set_state_range_mapped")
                and slot_reset_title_queue_producer_context.get("set_state_writes_requested_state_to_owner_4c")
                and slot_reset_title_queue_producer_context.get("set_state_validates_owner_48_bounds")
            ),
            "slot_reset_title_queue_set_state_callers_mapped": bool(
                slot_reset_title_queue_producer_context.get("set_state_callers_are_ingame_gate_and_menu_loop")
            ),
            "slot_reset_title_queue_menu_loop_sets_state5": bool(
                slot_reset_title_queue_producer_context.get("set_state_menu_loop_callers_use_state5")
            ),
            "slot_reset_title_queue_ingame_gate_sets_state0": bool(
                slot_reset_title_queue_producer_context.get("set_state_ingame_gate_caller_uses_state0")
            ),
            "slot_reset_menu_job_wait_sets_finish_state_11": bool(
                slot_reset_menu_job_wait_context.get("conditionally_sets_state_11")
            ),
            "slot_reset_menu_job_wait_pump_sequence_mapped": bool(
                slot_reset_menu_job_wait_context.get("submit_then_queue_order_mapped")
                and slot_reset_menu_job_wait_context.get("reuses_frame_delta_descriptor_for_submit_and_queue")
            ),
            "slot_reset_menu_job_wait_is_not_title_payload_enqueue": bool(
                slot_reset_menu_job_wait_context.get("does_not_directly_enqueue_title_accept_payload")
            ),
            "slot_reset_menu_job_wait_finish_gate_after_owner130_queue": bool(
                slot_reset_menu_job_wait_context.get("finish_gate_checked_only_after_owner130_queue")
            ),
            "slot_reset_set_state_helper_range_mapped": bool(
                slot_reset_menu_job_wait_context.get("set_state_begin_va") == "0x140b0d960"
                and slot_reset_menu_job_wait_context.get("set_state_end_va") == "0x140b0da69"
            ),
            "slot_reset_set_state_helper_writes_owner_4c": bool(
                slot_reset_menu_job_wait_context.get("set_state_helper_stores_edx_to_owner_4c")
                and slot_reset_menu_job_wait_context.get("set_state_helper_validates_current_plus_one_le_0xe")
            ),
            "slot_reset_set_state_helper_connects_menujobwait_and_finish": bool(
                slot_reset_menu_job_wait_context.get("set_state_helper_has_finish_callers")
            ),
            "slot_reset_timed_submit_helper_range_mapped": bool(
                slot_reset_timed_queue_context.get("submit_function_begin_va") == "0x140733f20"
                and slot_reset_timed_queue_context.get("submit_function_end_va") == "0x140733fe0"
            ),
            "slot_reset_timed_submit_iterates_existing_tasks": bool(
                slot_reset_timed_queue_context.get("submit_reads_owner_48_count")
                and slot_reset_timed_queue_context.get("submit_empty_returns_false_descriptor")
                and slot_reset_timed_queue_context.get("submit_iterates_entries_backwards")
                and slot_reset_timed_queue_context.get("submit_calls_entry_virtual_slot10_with_task_time")
                and slot_reset_timed_queue_context.get("submit_returns_true_descriptor_after_iteration")
            ),
            "slot_reset_timed_descriptor_vtables_mapped": bool(
                slot_reset_timed_queue_context.get("timed_descriptor_vtable_pair_mapped")
            ),
            "slot_reset_timed_submit_restores_descriptor_without_enqueue": bool(
                slot_reset_timed_queue_context.get("submit_restores_timed_descriptor_vtables_on_empty_and_after_iteration")
                and slot_reset_timed_queue_context.get("submit_is_pump_only_no_new_child_enqueue")
            ),
            "slot_reset_timed_queue_helper_range_mapped": bool(
                slot_reset_timed_queue_context.get("queue_function_begin_va") == "0x1407a9600"
                and slot_reset_timed_queue_context.get("queue_function_end_va") == "0x1407a9731"
            ),
            "slot_reset_timed_queue_consumes_existing_job_before_finish": bool(
                slot_reset_timed_queue_context.get("queue_captures_slot_and_descriptor")
                and slot_reset_timed_queue_context.get("queue_builds_timed_descriptor_from_arg_plus8")
                and slot_reset_timed_queue_context.get("queue_invokes_existing_job_virtual_slot10")
                and slot_reset_timed_queue_context.get("queue_checks_completion_with_7a9200")
                and slot_reset_timed_queue_context.get("queue_clears_slot_when_existing_job_consumed")
                and slot_reset_timed_queue_context.get("queue_releases_existing_job_and_clears_temp_descriptor")
            ),
            "slot_reset_timed_queue_check_semantics_mapped": bool(
                slot_reset_timed_queue_context.get("queue_check_function_begin_va") == "0x1407a9200"
                and slot_reset_timed_queue_context.get("queue_check_function_end_va") == "0x1407a9207"
                and slot_reset_timed_queue_context.get("queue_check_returns_descriptor_status_gt_one")
            ),
            "slot_reset_timed_queue_check_receives_virtual_result_descriptor": bool(
                slot_reset_timed_queue_context.get("queue_invokes_existing_job_virtual_slot10")
                and slot_reset_timed_queue_context.get("queue_passes_virtual_result_descriptor_to_check")
                and slot_reset_timed_queue_context.get("queue_checks_completion_with_7a9200")
            ),
            "slot_reset_timed_queue_terminal_check_gates_slot_clear": bool(
                slot_reset_timed_queue_context.get("queue_check_returns_descriptor_status_gt_one")
                and slot_reset_timed_queue_context.get("queue_terminal_check_gates_current_slot_clear")
                and slot_reset_timed_queue_context.get("queue_clears_slot_when_existing_job_consumed")
            ),
            "slot_reset_timed_queue_restores_descriptor_when_empty": bool(
                slot_reset_timed_queue_context.get("queue_restores_incoming_descriptor_without_creating_job_when_slot_empty")
                and slot_reset_timed_queue_context.get("queue_is_single_slot_consumer_not_enqueue")
            ),
            "title_accept_payload_range_mapped": bool(
                title_accept_payload_context.get("function_begin_va") == "0x1409b24e0"
                and title_accept_payload_context.get("function_end_va") == "0x1409b2cdb"
            ),
            "title_accept_payload_primary_enqueue_chain_mapped": bool(
                title_accept_payload_context.get("primary_trace_enqueue_sources_present")
                and title_accept_payload_context.get("primary_trace_link_source_present")
                and title_accept_payload_context.get("primary_trace_link_between_first_two_enqueues")
            ),
            "title_accept_payload_primary_chain_builder_mapped": bool(
                title_accept_payload_context.get("primary_chain_builder_source_present")
            ),
            "title_accept_payload_primary_link_uses_first_enqueue": bool(
                title_accept_payload_context.get("primary_link_uses_first_enqueue_result")
            ),
            "title_accept_payload_primary_link_feeds_second_enqueue": bool(
                title_accept_payload_context.get("primary_link_feeds_second_enqueue")
            ),
            "title_accept_payload_second_enqueue_saved_r15": bool(
                title_accept_payload_context.get("second_primary_enqueue_saved_r15")
            ),
            "title_accept_payload_state_chain_args_mapped": bool(
                title_accept_payload_context.get("state_chain_args_use_zero_and_add2")
            ),
            "title_accept_payload_descriptor_wrapper_enqueue_mapped": bool(
                title_accept_payload_context.get("state_descriptor_wrapper_uses_chain_result")
                and title_accept_payload_context.get("state_descriptor_wrapper_result_enqueued_saved_r14")
            ),
            "title_accept_payload_late_aux_builders_mapped": bool(
                title_accept_payload_context.get("late_aux_builder_sources_present")
            ),
            "title_accept_payload_late_aux_enqueues_saved": bool(
                title_accept_payload_context.get("late_aux_enqueues_saved_registers")
            ),
            "title_accept_payload_late_link_folds_aux_results": bool(
                title_accept_payload_context.get("late_link_chain_folds_aux_results_to_final_enqueue")
            ),
            "title_accept_payload_final_owner_builder_mapped": bool(
                title_accept_payload_context.get("final_owner_builder_source_present")
            ),
            "title_accept_payload_final_combiner_mapped": bool(
                title_accept_payload_context.get("final_combiner_source_present")
                and title_accept_payload_context.get("final_combiner_uses_late_link_result_and_owner_result")
            ),
            "title_accept_payload_final_enqueue_mapped": bool(
                title_accept_payload_context.get("final_combiner_result_enqueued")
            ),
            "title_accept_payload_final_wrapper_chains_prior_nodes": bool(
                title_accept_payload_context.get("final_enqueue_result_passed_to_wrapper")
                and title_accept_payload_context.get("final_wrapper_chains_r14_r15")
            ),
            "title_accept_payload_final_cleanup_mapped": bool(
                title_accept_payload_context.get("final_cleanup_after_chain_source_present")
            ),
            "title_accept_payload_final_combiner_body_mapped": bool(
                title_accept_payload_context.get("final_combiner_body_range_mapped")
                and title_accept_payload_context.get("final_combiner_calls_inner_and_attach")
                and title_accept_payload_context.get("final_combiner_captures_four_sources")
            ),
            "title_accept_payload_final_combiner_ownership_mapped": bool(
                title_accept_payload_context.get("final_combiner_inner_receives_four_locals")
                and title_accept_payload_context.get("final_combiner_clears_transferred_slots")
            ),
            "title_accept_payload_final_wrapper_body_mapped": bool(
                title_accept_payload_context.get("final_wrapper_body_range_mapped")
                and title_accept_payload_context.get("final_wrapper_calls_inner_with_flag")
                and title_accept_payload_context.get("final_wrapper_clears_source_slot")
            ),
            "title_accept_payload_branch_compose_body_mapped": bool(
                title_accept_payload_context.get("branch_compose_body_calls_inner")
            ),
            "title_accept_payload_branch_step_body_mapped": bool(
                title_accept_payload_context.get("branch_step_body_builds_and_attaches")
            ),
            "title_accept_payload_final_combiner_inner_mapped": bool(
                title_accept_payload_context.get("final_combiner_inner_short_circuits_all_empty")
                and title_accept_payload_context.get("final_combiner_inner_allocates_composite_node")
                and title_accept_payload_context.get("final_combiner_inner_appends_four_sources")
                and title_accept_payload_context.get("final_combiner_inner_stores_output_and_releases_inputs")
            ),
            "title_accept_payload_final_wrapper_inner_mapped": bool(
                title_accept_payload_context.get("final_wrapper_inner_transfers_source_and_flag")
            ),
            "title_accept_payload_branch_compose_inner_mapped": bool(
                title_accept_payload_context.get("branch_compose_inner_transfers_two_sources_and_flag")
            ),
            "title_accept_payload_branch_step_builder_inner_mapped": bool(
                title_accept_payload_context.get("branch_step_builder_reorders_inputs_on_false_flag")
                and title_accept_payload_context.get("branch_step_builder_calls_inner_with_three_locals")
            ),
            "title_accept_payload_branch_step_status_node_mapped": bool(
                title_accept_payload_context.get("branch_step_build_inner_allocates_status_node")
                and title_accept_payload_context.get("branch_step_build_inner_builds_payload_and_attaches")
            ),
            "title_accept_payload_branch_step_condition_chain_mapped": bool(
                title_accept_payload_context.get("branch_step_build_inner_builds_condition_chain")
                and title_accept_payload_context.get("branch_step_build_inner_cleans_condition_temp")
            ),
            "title_accept_payload_branch_step_status_matches_terminal_check": bool(
                title_accept_payload_context.get("branch_step_build_inner_allocates_status_node")
                and slot_reset_timed_queue_context.get("queue_check_returns_descriptor_status_gt_one")
                and slot_reset_timed_queue_context.get("queue_terminal_check_gates_current_slot_clear")
            ),
            "title_accept_payload_branch_step_vtable_chain_mapped": bool(
                title_accept_payload_context.get("branch_step_vtables_link_payload_to_status_sequence")
            ),
            "title_accept_payload_branch_step_payload_vslot2_mapped": bool(
                title_accept_payload_context.get("branch_step_payload_vslot2_sets_terminal_status")
                and title_accept_payload_context.get("branch_step_payload_vslot2_writes_condition_result")
            ),
            "title_accept_payload_branch_step_status_vslot2_mapped": bool(
                title_accept_payload_context.get("branch_step_status_vslot2_iterates_children_until_done")
                and title_accept_payload_context.get("branch_step_status_vslot2_stops_on_nonterminal_or_failed_child")
            ),
            "title_accept_payload_state_descriptor_mapped": bool(
                title_accept_payload_context.get("builder_state_descriptor_sources_present")
                and title_accept_payload_context.get("builds_title_accept_descriptor_vtables")
            ),
            "title_accept_payload_owner_fields_mapped": bool(
                title_accept_payload_context.get("uses_owner_payload_fields")
            ),
            "title_accept_payload_enqueue_fanout_mapped": bool(
                title_accept_payload_context.get("task_enqueue_fanout_count", 0) >= 8
                and title_accept_payload_context.get("task_enqueue_fanout_has_expected_late_sources")
            ),
            "title_accept_payload_late_link_chain_mapped": bool(
                title_accept_payload_context.get("late_chain_link_sources_present")
                and title_accept_payload_context.get("task_enqueue_fanout_has_expected_late_sources")
            ),
            "slot_reset_timed_submit_empty_descriptor_mapped": bool(
                slot_reset_timed_queue_context.get("submit_empty_returns_false_descriptor")
            ),
            "slot_reset_menujobwait_queue_before_finish_gate_mapped": bool(
                slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_toggles_global_and_queues_owner108")
                and finish_gate_synchronization_context.get("ending_menu_job_wait_source") == "0x140ae546f"
                and slot_reset_menu_job_wait_context.get("queues_owner_130_timed_task")
                and finish_gate_synchronization_context.get("title_menu_job_wait_source") == "0x140b0d526"
            ),
            "slot_reset_menujobwait_queue_slots_linked_across_title_and_ending": bool(
                slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_submits_owner_b8_timed_task")
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_toggles_global_and_queues_owner108")
                and slot_reset_menu_job_wait_context.get("submits_owner_e0_timed_task")
                and slot_reset_menu_job_wait_context.get("queues_owner_130_timed_task")
                and finish_gate_synchronization_context.get("ending_menu_job_wait_source") == "0x140ae546f"
                and finish_gate_synchronization_context.get("title_menu_job_wait_source") == "0x140b0d526"
            ),
            "slot_reset_finish_gate_global_has_broad_refs": bool(
                slot_reset_menu_job_wait_context.get("finish_gate_ref_count", 0) >= 15
            ),
            "slot_reset_finish_gate_links_menujobwait_and_finish": bool(
                slot_reset_menu_job_wait_context.get("finish_gate_refs_include_menu_job_wait")
                and slot_reset_menu_job_wait_context.get("finish_gate_refs_include_finish_handler")
            ),
            "slot_reset_finish_gate_links_save_request_family": bool(
                slot_reset_menu_job_wait_context.get("finish_gate_refs_include_save_request_gate_family")
            ),
            "slot_reset_end_flow_wait_range_mapped": bool(
                slot_reset_end_flow_wait_context.get("function_begin_va") == "0x140b0ccc0"
                and slot_reset_end_flow_wait_context.get("function_end_va") == "0x140b0cd62"
            ),
            "slot_reset_end_flow_wait_probe_and_reset_mapped": bool(
                slot_reset_end_flow_wait_context.get("captures_owner_rbx")
                and slot_reset_end_flow_wait_context.get("sets_global_job_active_flag_6b0")
                and slot_reset_end_flow_wait_context.get("probes_owner_c0")
                and slot_reset_end_flow_wait_context.get("probe_success_calls_reset_with_zero")
            ),
            "slot_reset_end_flow_wait_branch_gate_mapped": bool(
                slot_reset_end_flow_wait_context.get("branches_on_end_flow_gate")
            ),
            "slot_reset_end_flow_wait_finish_gate_to_state11_mapped": bool(
                slot_reset_end_flow_wait_context.get("checks_finish_gate_before_state11")
                and slot_reset_end_flow_wait_context.get("sets_state_11_via_helper")
                and slot_reset_end_flow_wait_context.get("returns_without_state_change_when_finish_gate_clear")
            ),
            "slot_reset_end_flow_probe_range_mapped": bool(
                slot_reset_end_flow_wait_context.get("probe_range_mapped")
            ),
            "slot_reset_end_flow_probe_owner_c0_semantics_mapped": bool(
                slot_reset_end_flow_wait_context.get("probe_reads_owner_c0_plus8")
                and slot_reset_end_flow_wait_context.get("probe_null_returns_true")
                and slot_reset_end_flow_wait_context.get("probe_nonnull_calls_virtual_slot20")
                and slot_reset_end_flow_wait_context.get("probe_returns_virtual_truthiness")
            ),
            "slot_reset_end_flow_reset_clears_gameman_b5e": bool(
                slot_reset_end_flow_wait_context.get("probe_success_calls_reset_with_zero")
                and slot_reset_end_flow_wait_context.get("reset_sets_game_man_b5e_from_cl")
            ),
            "game_man_b5e_direct_accessors_mapped": bool(
                {
                    "0x140679845",
                    "0x14067a1b7",
                    "0x14067ae97",
                    "0x14067e253",
                }.issubset(b5e_direct_sources)
            ),
            "game_man_b5e_getter_refs_include_move_map_sites": bool(
                {
                    "0x140af379d",
                    "0x140afaad0",
                    "0x140afd2a9",
                    "0x140afd9c6",
                }.issubset(b5e_getter_sources)
            ),
            "game_man_b5e_getter_gates_continue_request_path": any(
                context.get("source_va") == "0x140afaad0"
                and (context.get("branches_on_getter_result") or context.get("cmov_uses_getter_result"))
                and context.get("nearby_request_save")
                and context.get("nearby_save_request_profile")
                for context in game_man_b5e_context.get("getter_call_contexts", [])
            ),
            "game_man_b5e_setter_refs_include_move_map_and_title_reset": bool(
                {
                    "0x140af5a00",
                    "0x140afb173",
                    "0x140b0cd1c",
                    "0x140b0e77b",
                }.issubset(b5e_setter_sources)
            ),
            "game_man_b5e_setter_clear_paths_mapped": bool(
                any(
                    context.get("source_va") == "0x140afb173"
                    and context.get("move_map_region")
                    and context.get("passes_zero_to_setter")
                    and context.get("nearby_set_save_slot_wrapper_67a820")
                    for context in game_man_b5e_context.get("setter_call_contexts", [])
                )
                and any(
                    context.get("source_va") == "0x140b0cd1c"
                    and context.get("title_step_region")
                    and context.get("passes_zero_to_setter")
                    for context in game_man_b5e_context.get("setter_call_contexts", [])
                )
            ),
            "game_man_b5e_setter_set_path_mapped": bool(
                any(
                    context.get("source_va") == "0x140af5a00"
                    and context.get("move_map_region")
                    and context.get("passes_one_to_setter")
                    for context in game_man_b5e_context.get("setter_call_contexts", [])
                )
                and any(
                    context.get("source_va") == "0x140b0e77b"
                    and context.get("title_step_region")
                    and context.get("passes_one_to_setter")
                    for context in game_man_b5e_context.get("setter_call_contexts", [])
                )
            ),
            "game_man_b5e_bulk_clear_called_from_move_map": bool(
                b5e_bulk_clear_sources == {"0x140afca1d"}
            ),
            "slot_reset_end_flow_tail_branches_range_mapped": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("function_begin_va") == "0x140b0e650"
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("function_end_va") == "0x140b0e780"
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("function_begin_va") == "0x140b0e780"
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("function_end_va") == "0x140b0e985"
            ),
            "slot_reset_end_flow_success_gate_selects_tail_branch": bool(
                slot_reset_end_flow_wait_context.get("branch_gate_global_va") == "0x143d6f9c0"
                and slot_reset_end_flow_wait_context.get("branch_gate_zero_jumps_e650")
                and slot_reset_end_flow_wait_context.get("branch_gate_nonzero_jumps_e780")
                and slot_reset_end_flow_wait_context.get("probe_success_branches_before_finish_gate")
            ),
            "slot_reset_end_flow_e650_sets_state5_and_b5e": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("sets_owner_3e1_active")
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("sets_state_5_via_helper")
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("writes_owner_bc_from_eax")
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("passes_one_to_b5e_setter")
            ),
            "slot_reset_end_flow_e650_restores_selected_value_from_counter": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("reads_counter_d0")
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("reads_selected_value_d4")
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("passes_one_to_selected_value_set")
            ),
            "slot_reset_end_flow_e780_sets_state5_without_b5e": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("sets_owner_3e1_active")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("sets_owner_3e0_complete")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("sets_state_5_via_helper")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("writes_owner_bc_from_eax")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("does_not_call_b5e_setter")
            ),
            "slot_reset_end_flow_e780_resets_selected_value_and_counter": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("caps_counter_120_to_270f")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("passes_zero_to_selected_value_set")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("calls_counter_reset_before_selected_value_set")
            ),
            "slot_reset_end_flow_tail_branches_return_to_playgame": bool(
                any(
                    entry.get("entry_index") == 5
                    and entry.get("handler_va") == "0x140b0d5b0"
                    and entry.get("label_text") == "TitleStep::STEP_PlayGame"
                    for entry in slot_reset_state_table_init_context.get("entries", [])
                )
                and slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("sets_state_5_via_helper")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("sets_state_5_via_helper")
            ),
            "slot_reset_playgame_handler_range_mapped": bool(
                slot_reset_play_game_context.get("function_begin_va") == "0x140b0d5b0"
                and slot_reset_play_game_context.get("function_end_va") == "0x140b0d832"
                and slot_reset_play_game_context.get("tail_function_begin_va") == "0x140b0d850"
                and slot_reset_play_game_context.get("tail_function_end_va") == "0x140b0d959"
            ),
            "slot_reset_playgame_submits_owner_bc_job": bool(
                slot_reset_play_game_context.get("prepares_stack_payload_from_arg_flag")
                and slot_reset_play_game_context.get("submits_owner_bc_and_owner_2e8_job")
                and slot_reset_play_game_context.get("cleans_submitted_payload_virtual_slot68")
            ),
            "slot_reset_playgame_consumes_tail_active_flag": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("sets_owner_3e1_active")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("sets_owner_3e1_active")
                and slot_reset_play_game_context.get("consumes_owner_3e1_flag_and_clears_it")
            ),
            "slot_reset_playgame_updates_nonnegative_save_slot_global": bool(
                slot_reset_play_game_context.get("gets_save_slot_and_stores_nonnegative_to_global_1200")
            ),
            "slot_reset_playgame_menu_object_probe_path_mapped": bool(
                slot_reset_play_game_context.get("checks_global_menu_object_b0_probes")
            ),
            "slot_reset_playgame_tail_increments_state_counter": bool(
                slot_reset_play_game_context.get("tail_jumps_to_state_increment_helper")
                and slot_reset_play_game_context.get("tail_increments_owner_4c_and_bounds_owner_48")
            ),
            "slot_reset_playgame_submit_helper_range_mapped": bool(
                slot_reset_play_game_submit_context.get("function_begin_va") == "0x140aebdc0"
                and slot_reset_play_game_submit_context.get("function_end_va") == "0x140aebf8b"
            ),
            "slot_reset_playgame_submit_args_mapped": bool(
                slot_reset_play_game_submit_context.get("captures_rcx_owner_job_rsi")
                and slot_reset_play_game_submit_context.get("captures_rdx_slot_ptr_rdi")
                and slot_reset_play_game_submit_context.get("captures_r9_payload_rbp")
                and slot_reset_play_game_submit_context.get("stores_r8d_stack_arg")
            ),
            "slot_reset_playgame_submit_rejects_minus_one_slot": bool(
                slot_reset_play_game_submit_context.get("returns_early_when_slot_minus_one")
            ),
            "slot_reset_playgame_submit_stores_validated_load_pair": bool(
                slot_reset_play_game_submit_context.get("sets_owner_job_d8_active")
                and slot_reset_play_game_submit_context.get("copies_requested_slot_to_stack_for_helpers")
                and slot_reset_play_game_submit_context.get("calls_selected_value_validate_then_load_pair")
                and slot_reset_play_game_submit_context.get("stores_load_pair_to_owner_job_100_104")
            ),
            "slot_reset_playgame_submit_appends_payload_vector": bool(
                slot_reset_play_game_submit_context.get("appends_payload_vector_to_owner_job_b35f0")
                and slot_reset_play_game_submit_context.get("grows_owner_job_vector_when_full")
            ),
            "slot_reset_selected_value_get_set_mapped": bool(
                slot_reset_selected_value_context.get("get_ac0_reads_global_slot")
                and slot_reset_selected_value_context.get("set_b60_b5f_writes_value_and_flag")
            ),
            "slot_reset_selected_value_validate_flags_mapped": bool(
                slot_reset_selected_value_context.get("validate_function_begin_va") == "0x14067aec0"
                and slot_reset_selected_value_context.get("validate_function_end_va") == "0x14067af5b"
                and slot_reset_selected_value_context.get("validate_checks_required_globals")
                and slot_reset_selected_value_context.get("validate_copies_input_value_to_stack")
                and slot_reset_selected_value_context.get("validate_prepares_selected_value")
                and slot_reset_selected_value_context.get("validate_queries_12d_12e_flags")
                and slot_reset_selected_value_context.get("validate_sets_bcd_bce_and_bcc")
            ),
            "slot_reset_selected_value_pair_normalization_mapped": bool(
                slot_reset_selected_value_context.get("pair_function_begin_va") == "0x14067abd0"
                and slot_reset_selected_value_context.get("pair_function_end_va") == "0x14067ac24"
                and slot_reset_selected_value_context.get("pair_copies_input_to_output")
                and slot_reset_selected_value_context.get("pair_requires_b28_clear_and_slot_not_minus_one")
                and slot_reset_selected_value_context.get("pair_rewrites_special_slot_range")
                and slot_reset_selected_value_context.get("pair_stores_output_to_game_man_14")
            ),
            "slot_reset_playgame_owner_bc_to_gameman14_flow_mapped": bool(
                slot_reset_end_flow_tail_branch_context.get("branch_e650", {}).get("writes_owner_bc_from_eax")
                and slot_reset_end_flow_tail_branch_context.get("branch_e780", {}).get("writes_owner_bc_from_eax")
                and slot_reset_play_game_context.get("submits_owner_bc_and_owner_2e8_job")
                and slot_reset_play_game_submit_context.get("stores_load_pair_to_owner_job_100_104")
                and slot_reset_selected_value_context.get("pair_stores_output_to_game_man_14")
            ),
            "slot_reset_selected_value_direct_field_accesses_mapped": bool(
                {"0x140678ca7", "0x141934e6e", "0x14199a1b5"}.issubset(
                    selected_field_sources.get("ac0_global_slot_read", set())
                )
                and selected_field_sources.get("b60_selected_value_write") == {"0x14067ac69"}
                and selected_field_sources.get("b5f_selected_value_flag_write") == {"0x14067ac6f"}
                and selected_field_sources.get("b5f_selected_value_flag_read") == {"0x14067a077"}
                and selected_field_sources.get("b28_pair_normalization_gate_read") == {"0x14067abe4"}
                and selected_field_sources.get("bcd_validate_12d_write") == {"0x14067af25"}
                and selected_field_sources.get("bce_validate_12e_write") == {"0x14067af49"}
            ),
            "slot_reset_selected_value_validate_ready_flag_lifecycle_mapped": bool(
                selected_field_sources.get("bcc_validate_ready_write_one") == {"0x14067af4f"}
                and selected_field_sources.get("bcc_validate_ready_write_zero") == {"0x14067af6c"}
            ),
            "slot_reset_selected_value_getter_callers_mapped": bool(
                {"0x1409a9272", "0x140aecf68", "0x140b0d690"}.issubset(
                    {str(source) for source in slot_reset_selected_value_field_access_context.get("get_slot_call_sources", [])}
                )
                and slot_reset_selected_value_field_access_context.get("b5f_getter_call_sources") == ["0x140594918"]
            ),
            "slot_reset_selected_value_ac0_callers_store_or_gate_global_slot": bool(
                slot_reset_selected_value_caller_context.get("ac0_ui_store_9a9272", {}).get(
                    "stores_nonnegative_ac0_to_global_1200"
                )
                and slot_reset_selected_value_caller_context.get("ac0_playgame_b0d690", {}).get(
                    "stores_nonnegative_ac0_to_global_1200"
                )
                and slot_reset_selected_value_caller_context.get("ac0_title_submit_aecf68", {}).get(
                    "calls_post_requested_slot_after_ac0_gate"
                )
            ),
            "slot_reset_selected_value_b5f_caller_forces_slot_zero_and_b5e": bool(
                slot_reset_selected_value_caller_context.get("b5f_load_selection_594918", {}).get("b5f_result_saved_in_ebx")
                and slot_reset_selected_value_caller_context.get("b5f_load_selection_594918", {}).get("b5f_true_sets_slot_zero")
                and slot_reset_selected_value_caller_context.get("b5f_load_selection_594918", {}).get("b5f_true_sets_b5e_one")
                and slot_reset_selected_value_caller_context.get("b5f_load_selection_594918", {}).get(
                    "always_normalizes_via_pair_helper"
                )
            ),
            "slot_reset_end_flow_branch_gate_refs_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("ref_sources")
                == ["0x140ae5cd6", "0x140ae6350", "0x140ae6360", "0x140b0cd21"]
            ),
            "slot_reset_end_flow_branch_gate_state_table_init_mapped": bool(
                slot_reset_end_flow_branch_gate_table_init_context.get("function_begin_va") == "0x1400a2e10"
                and slot_reset_end_flow_branch_gate_table_init_context.get("function_end_va") == "0x1400a2eb7"
                and slot_reset_end_flow_branch_gate_table_init_context.get("table_base_va") == "0x143d6f9d0"
                and slot_reset_end_flow_branch_gate_table_init_context.get("zero_size_bytes") == 0x50
            ),
            "slot_reset_end_flow_branch_gate_state_table_entries_mapped": bool(
                {
                    0: "0x140ae5cd0",
                    1: "0x140ae5d10",
                    2: "0x140ae54a0",
                    3: "0x140ae5390",
                    4: "0x140ae5380",
                }.items()
                <= {
                    int(entry.get("entry_index")): str(entry.get("handler_va"))
                    for entry in slot_reset_end_flow_branch_gate_table_init_context.get("entries", [])
                }.items()
            ),
            "slot_reset_end_flow_branch_gate_state_table_labels_mapped": bool(
                {
                    0: "EndingStep::STEP_PlayEndingMovie",
                    1: "EndingStep::STEP_PlayEndingMovieWait",
                    2: "EndingStep::STEP_NextLapQuestion",
                    3: "EndingStep::STEP_MenuJobWait",
                    4: "EndingStep::STEP_Finish",
                }.items()
                <= {
                    int(entry.get("entry_index")): str(entry.get("label_text"))
                    for entry in slot_reset_end_flow_branch_gate_table_init_context.get("entries", [])
                }.items()
            ),
            "slot_reset_end_flow_branch_gate_state_table_links_builder_and_countdown": bool(
                any(
                    entry.get("entry_index") == 0 and entry.get("handler_va") == "0x140ae5cd0"
                    for entry in slot_reset_end_flow_branch_gate_table_init_context.get("entries", [])
                )
                and any(
                    entry.get("entry_index") == 2 and entry.get("handler_va") == "0x140ae54a0"
                    for entry in slot_reset_end_flow_branch_gate_table_init_context.get("entries", [])
                )
            ),
            "slot_reset_end_flow_branch_gate_wait_handler_mapped": bool(
                slot_reset_end_flow_branch_gate_state_handlers_context.get("wait_function_begin_va") == "0x140ae5d10"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("wait_function_end_va") == "0x140ae5d4b"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("wait_probe_false_advances_countdown_done")
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("wait_finish_gate_sets_stage4")
            ),
            "slot_reset_end_flow_branch_gate_menujobwait_handler_mapped": bool(
                slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_function_begin_va") == "0x140ae5390"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_function_end_va") == "0x140ae5492"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_builds_timed_task_from_arg_plus8")
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_toggles_global_and_queues_owner108")
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("menu_job_wait_finish_gate_sets_stage4")
            ),
            "slot_reset_end_flow_branch_gate_finish_handler_mapped": bool(
                slot_reset_end_flow_branch_gate_state_handlers_context.get("finish_function_begin_va") == "0x140ae5380"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("finish_function_end_va") == "0x140ae5388"
                and slot_reset_end_flow_branch_gate_state_handlers_context.get("finish_sets_owner_4c_minus_one")
            ),
            "finish_gate_synchronization_refs_mapped": bool(
                finish_gate_synchronization_context.get("finish_gate_va") == "0x143d856a0"
                and finish_gate_synchronization_context.get("ref_count") == 21
            ),
            "finish_gate_links_ending_and_title_stage_machines": bool(
                finish_gate_synchronization_context.get("ending_wait_source") == "0x140ae5d2a"
                and finish_gate_synchronization_context.get("ending_menu_job_wait_source") == "0x140ae546f"
                and finish_gate_synchronization_context.get("title_end_flow_wait_source") == "0x140b0cd41"
                and finish_gate_synchronization_context.get("title_menu_job_wait_source") == "0x140b0d526"
            ),
            "finish_gate_links_save_request_and_movemap_dispatch": bool(
                finish_gate_synchronization_context.get("save_request_gate_sources")
                == ["0x14067a3d4", "0x14067a3e4", "0x14067a509", "0x14067a66a"]
                and finish_gate_synchronization_context.get("move_map_dispatch_source") == "0x140afb8e1"
            ),
            "slot_reset_end_flow_branch_gate_countdown_clear_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("clear_countdown_function_begin_va") == "0x140ae5cd0"
                and slot_reset_end_flow_branch_gate_context.get("clear_countdown_function_end_va") == "0x140ae5d0f"
                and slot_reset_end_flow_branch_gate_context.get("clear_countdown_clears_gate_first")
                and slot_reset_end_flow_branch_gate_context.get("clear_countdown_uses_owner_110_countdown")
                and slot_reset_end_flow_branch_gate_context.get("clear_countdown_jumps_done_when_zero")
            ),
            "slot_reset_end_flow_branch_gate_countdown_done_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("done_function_begin_va") == "0x140ae5d50"
                and slot_reset_end_flow_branch_gate_context.get("done_function_end_va") == "0x140ae5e59"
                and slot_reset_end_flow_branch_gate_context.get("done_increments_owner_4c")
                and slot_reset_end_flow_branch_gate_context.get("done_bounds_owner_48_plus_one_le_6")
            ),
            "slot_reset_end_flow_branch_gate_tiny_wrappers_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("tiny_clear_wrapper_clears_gate")
                and slot_reset_end_flow_branch_gate_context.get("tiny_set_wrapper_sets_gate")
                and slot_reset_end_flow_branch_gate_context.get("endflow_read_ref_source") == "0x140b0cd21"
            ),
            "slot_reset_end_flow_branch_gate_table_entries_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("tiny_set_table_entry_va") == "0x142b5af58"
                and slot_reset_end_flow_branch_gate_context.get("tiny_clear_table_entry_va") == "0x142b5af90"
            ),
            "slot_reset_end_flow_branch_gate_action_slot_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("tiny_set_table_entry_va") == "0x142b5af58"
                and int("0x142b5af58", 16) - int("0x142b5af48", 16) == 0x10
                and slot_reset_end_flow_branch_gate_context.get("tiny_clear_table_entry_va") == "0x142b5af90"
                and int("0x142b5af90", 16) - int("0x142b5af80", 16) == 0x10
            ),
            "slot_reset_end_flow_branch_gate_builder_range_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("builder_function_begin_va") == "0x140ae54a0"
                and slot_reset_end_flow_branch_gate_context.get("builder_function_end_va") == "0x140ae5cc4"
            ),
            "slot_reset_end_flow_branch_gate_builder_installs_set_clear_tables": bool(
                [ref.get("source_va") for ref in slot_reset_end_flow_branch_gate_context.get("builder_table_refs", {}).get("set_table_plus8_142b5af48", [])]
                == ["0x140ae565e"]
                and [ref.get("source_va") for ref in slot_reset_end_flow_branch_gate_context.get("builder_table_refs", {}).get("clear_table_142b5af80", [])]
                == ["0x140ae55f1"]
                and [ref.get("source_va") for ref in slot_reset_end_flow_branch_gate_context.get("builder_table_refs", {}).get("sibling_table_plus8_142b5afb8", [])]
                == ["0x140ae5583"]
            ),
            "slot_reset_end_flow_branch_gate_builder_enqueues_three_descriptors": bool(
                len(slot_reset_end_flow_branch_gate_context.get("builder_calls_by_target", {}).get("selector_builder_chain_key_7a91e0", [])) >= 3
                and len(slot_reset_end_flow_branch_gate_context.get("builder_calls_by_target", {}).get("slot_reset_branch_gate_descriptor_wrapper_744a60", [])) >= 3
                and len(slot_reset_end_flow_branch_gate_context.get("builder_calls_by_target", {}).get("task_enqueue_7a7b60", [])) >= 3
            ),
            "slot_reset_end_flow_branch_gate_builder_resource_ids_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("builder_has_resource_ids_6ddd1_6ddd0_1061")
                and slot_reset_end_flow_branch_gate_context.get("builder_has_timed_param_blocks_64_2_1")
            ),
            "slot_reset_end_flow_branch_gate_builder_chain_helpers_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("resource_75fbd0")
                == ["0x140ae56b2", "0x140ae570c"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("timed_job_7b73d0")
                == ["0x140ae56eb", "0x140ae573e"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("chain_start_ae5180")
                == ["0x140ae576c"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("chain_compose_78e0e0")
                == ["0x140ae577f", "0x140ae583a"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("chain_step_7927d0")
                == ["0x140ae5790"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("resource_762d50")
                == ["0x140ae57b1"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("job_7bae20")
                == ["0x140ae57c6"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("condition_ae5230")
                == ["0x140ae5827"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("chain_step_7928a0")
                == ["0x140ae5848"]
                and slot_reset_end_flow_branch_gate_context.get("builder_chain_helper_sources", {}).get("submit_ae6520")
                == ["0x140ae5883"]
            ),
            "slot_reset_end_flow_branch_gate_submit_helper_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("submit_function_begin_va") == "0x140ae6520"
                and slot_reset_end_flow_branch_gate_context.get("submit_function_end_va") == "0x140ae65ff"
                and slot_reset_end_flow_branch_gate_context.get("submit_captures_owner_and_chain_ptr")
                and slot_reset_end_flow_branch_gate_context.get("submit_attaches_chain_to_owner_108")
                and slot_reset_end_flow_branch_gate_context.get("submit_advances_owner_stage_3")
                and slot_reset_end_flow_branch_gate_context.get("submit_clears_source_chain_ptr")
            ),
            "slot_reset_end_flow_branch_gate_stage_helper_mapped": bool(
                slot_reset_end_flow_branch_gate_context.get("stage_function_begin_va") == "0x140ae5e60"
                and slot_reset_end_flow_branch_gate_context.get("stage_function_end_va") == "0x140ae5f69"
                and slot_reset_end_flow_branch_gate_context.get("stage_writes_owner_4c_from_edx")
                and slot_reset_end_flow_branch_gate_context.get("stage_bounds_owner_48_plus_one_le_6")
            ),
        },
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"static_re_evidence_path={output}")
    for key, value in evidence["summary"].items():
        print(f"static_re_{key}={int(bool(value))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
