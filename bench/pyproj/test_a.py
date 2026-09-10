import pytest


def add(a, b):
    return a + b


def test_add_ok():
    assert add(1, 2) == 3


def test_add_neg():
    assert add(-1, -1) == -2


def test_div_fail():
    total = 10
    parts = 0
    assert total / max(parts, 1) == 5


def test_str_fail():
    assert "prism".upper() == "PRISN"


def test_list_fail():
    assert [1, 2, 3, 4] == [1, 2, 4, 3]


def test_ok_again():
    assert add(2, 2) == 4


@pytest.mark.skip(reason="not implemented yet")
def test_skipped():
    assert False
