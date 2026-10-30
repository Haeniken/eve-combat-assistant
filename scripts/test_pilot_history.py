"""Research evidence checks; no network or private data."""
import copy
from pathlib import Path
import unittest

from collect_pilot_history import Collector
from build_pilot_catalog import offensive


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.collector = Collector(Path('.'), {'ships': {
            '1': {'group': 25}, '2': {'group': 25}, '3': {'group': 29},
        }, 'combat_items': [10, 11]}, {})
        self.mail = {'killmail_id': 123, 'killmail_time': '2026-10-29T12:00:00Z',
                     'victim': {'character_id': 200, 'ship_type_id': 2,
                                'items': [{'item_type_id': 10, 'flag': 27}]},
                     'attackers': [{'character_id': 100, 'ship_type_id': 1, 'damage_done': 100}]}

    def evidence(self, mail=None):
        row, reason = self.collector.evidence(mail or self.mail, '20261029')
        self.assertEqual(reason, 'eligible')
        return row

    def test_high_slot_weapon_is_fitted_and_cargo_is_not(self):
        self.assertEqual(self.evidence()[10], 1)
        for flag in [5, 11, 19, 35]:
            self.mail['victim']['items'][0]['flag'] = flag
            self.assertEqual(self.evidence()[10], 0)
        self.mail['victim']['items'][0]['flag'] = 34
        self.assertEqual(self.evidence()[10], 1)

    def test_combat_drone_bay_and_missing_items_are_distinct(self):
        self.mail['victim']['items'] = [{'item_type_id': 11, 'flag': 87}]
        self.assertEqual(self.evidence()[10], 1)
        self.mail['victim'].pop('items')
        self.assertEqual(self.evidence()[10], -1)
        self.mail['victim']['items'] = []
        self.assertEqual(self.evidence()[10], 0)

    def test_pvp_damage_and_npc_damage_remain_separate(self):
        self.mail['attackers'].append({'damage_done': 900})
        row = self.evidence()
        self.assertEqual(row[11:13], (100, 900))

    def test_self_kills_second_player_and_capsules_are_excluded(self):
        cases = []
        mail = copy.deepcopy(self.mail);mail['victim']['character_id'] = 100;cases.append(mail)
        mail = copy.deepcopy(self.mail);mail['victim']['ship_type_id'] = 3;cases.append(mail)
        mail = copy.deepcopy(self.mail);mail['attackers'].append({'character_id': 300, 'ship_type_id': 1});cases.append(mail)
        for mail in cases:
            self.assertIsNone(self.collector.evidence(mail, '20261029')[0])

    def test_wrong_day_does_not_enter_training(self):
        with self.assertRaises(ValueError):
            self.collector.evidence(self.mail, '20261028')

    def test_utility_launchers_scripts_and_smartbombs_are_classified(self):
        def item(name, group=0, damage=0):
            return {'category': 7, 'group': group, 'name': name,
                    'attributes': {'emDamage': damage}}
        self.assertFalse(offensive(item('Expanded Probe Launcher II')))
        self.assertFalse(offensive(item('Tracking Computer II')))
        self.assertTrue(offensive(item('Small EMP Smartbomb II', 72, 70)))
        self.assertTrue(offensive(item('Rapid Light Missile Launcher II')))
        self.assertTrue(offensive(item('Light Entropic Disintegrator II')))


if __name__ == '__main__':
    unittest.main()
