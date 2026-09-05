from __future__ import annotations

import unittest

from shapely.geometry import LineString, Polygon
from shapely.strtree import STRtree

from isophilly_ingest.frontage_audit import (
    EdgeCandidate,
    HouseRange,
    ParsedAddress,
    StreetIndex,
    StreetSegment,
    _candidate_for_polygon,
    frontage_edge_for_polygon,
    normalize_street_label,
    parse_address,
    parse_street_range,
    ring_edges,
    select_frontage_edge,
)


def street(object_id: int = 1) -> StreetSegment:
    return StreetSegment(
        object_id=object_id,
        street_label="N 18TH ST",
        street_class=5,
        ranges=(HouseRange(1500, 1598),),
        geometry=LineString(((0, 0), (10, 0))),
    )


def candidate(edge: LineString, distance: float, object_id: int = 1) -> EdgeCandidate:
    return EdgeCandidate(edge, street(object_id), distance, 0.0)


class FrontageAddressTests(unittest.TestCase):
    def test_normalizes_only_audited_direction_and_type_synonyms(self) -> None:
        self.assertEqual(normalize_street_label("north 18th street"), "N 18TH ST")
        self.assertEqual(normalize_street_label("N 18TH ST."), "N 18TH ST")
        self.assertEqual(normalize_street_label("N 18TH STR"), "N 18TH STR")

    def test_parses_number_range_and_hash_unit_conservatively(self) -> None:
        self.assertEqual(
            parse_address("1500-1504 North 18th Street # 2B"),
            ParsedAddress(HouseRange(1500, 1504), "N 18TH ST"),
        )
        self.assertIsNone(parse_address("1500-1503 N 18TH ST"))
        self.assertIsNone(parse_address("0 N 18TH ST"))
        self.assertIsNone(parse_address("1500 N 18TH ST APT 2"))

    def test_range_and_parity_require_one_valid_street_side(self) -> None:
        address = ParsedAddress(HouseRange(1502, 1504), "N 18TH ST")
        self.assertTrue(HouseRange(1500, 1598).fits(address.house_range))
        self.assertFalse(HouseRange(1501, 1599).fits(address.house_range))
        self.assertEqual(parse_street_range(1598, 1500), HouseRange(1500, 1598))
        self.assertIsNone(parse_street_range(1500, 1599))


class FrontageSelectionTests(unittest.TestCase):
    def test_selects_single_edge(self) -> None:
        result = select_frontage_edge((candidate(LineString(((0, 4), (10, 4))), 4.0),))
        self.assertIsNotNone(result.candidate)
        self.assertIsNone(result.rejected_reason)

    def test_corner_and_near_tie_are_rejected(self) -> None:
        first = LineString(((0, 4), (10, 4)))
        second = LineString(((10, 4), (10, 14)))
        result = select_frontage_edge((candidate(first, 4.0), candidate(second, 5.99)))
        self.assertIsNone(result.candidate)
        self.assertEqual(result.rejected_reason, "ambiguous_edge_tie")

    def test_two_metres_closer_selects_nearest(self) -> None:
        first = LineString(((0, 4), (10, 4)))
        second = LineString(((10, 5), (10, 15)))
        result = select_frontage_edge((candidate(second, 6.0), candidate(first, 4.0)))
        self.assertIsNotNone(result.candidate)
        assert result.candidate is not None
        self.assertEqual(result.candidate.edge_key, candidate(first, 4.0).edge_key)

    def test_ring_winding_and_start_order_do_not_change_edge_selection(self) -> None:
        clockwise = Polygon(((0, 4), (10, 4), (10, 14), (0, 14), (0, 4)))
        counter_clockwise = Polygon(((10, 14), (10, 4), (0, 4), (0, 14), (10, 14)))
        target = LineString(((0, 4), (10, 4)))
        first = select_frontage_edge(
            tuple(
                candidate(edge, 4.0 if edge.equals(target) else 9.0)
                for edge in ring_edges(clockwise)
            )
        )
        second = select_frontage_edge(
            tuple(
                candidate(edge, 4.0 if edge.equals(target) else 9.0)
                for edge in ring_edges(counter_clockwise)
            )
        )
        self.assertIsNotNone(first.candidate)
        self.assertIsNotNone(second.candidate)
        assert first.candidate is not None
        assert second.candidate is not None
        self.assertEqual(first.candidate.edge_key, second.candidate.edge_key)

    def test_selects_the_actual_final_simplified_ring_edge(self) -> None:
        final_polygon = Polygon(((0, 4), (5, 4), (5, 14), (0, 14), (0, 4))).simplify(
            0.35, preserve_topology=True
        )
        assert isinstance(final_polygon, Polygon)
        street_segment = street()
        index = StreetIndex(
            (street_segment,), STRtree([street_segment.geometry]), {"N 18TH ST": frozenset({0})}
        )
        selected = frontage_edge_for_polygon(
            final_polygon,
            ParsedAddress(HouseRange(1500, 1500), "N 18TH ST"),
            index,
        )
        self.assertEqual(selected, 0)

    def test_unrepresentable_edge_index_fails_closed_to_unknown(self) -> None:
        points = tuple((float(index), 0.0) for index in range(255))
        polygon = Polygon((*points, (254.0, 1.0), (259.0, 1.0), (259.0, 14.0), (0.0, 14.0)))
        target = LineString(((254.0, 1.0), (259.0, 1.0)))
        street_segment = StreetSegment(
            object_id=1,
            street_label="N 18TH ST",
            street_class=5,
            ranges=(HouseRange(1500, 1598),),
            geometry=LineString(((254.0, -3.0), (259.0, -3.0))),
        )
        index = StreetIndex(
            (street_segment,), STRtree([street_segment.geometry]), {"N 18TH ST": frozenset({0})}
        )
        nearby = index.nearby_matching(polygon, ParsedAddress(HouseRange(1500, 1500), "N 18TH ST"))
        selection, _ = _candidate_for_polygon(polygon, nearby)
        self.assertIsNotNone(selection.candidate)
        assert selection.candidate is not None
        self.assertTrue(selection.candidate.edge.equals(target))
        self.assertIsNone(
            frontage_edge_for_polygon(
                polygon,
                ParsedAddress(HouseRange(1500, 1500), "N 18TH ST"),
                index,
            )
        )
