import pytest


class Repo:
    def __init__(self):
        self.items = {}

    def get(self, k):
        return self.items[k]


@pytest.fixture
def repo():
    return Repo()


def test_get_missing(repo):
    with pytest.raises(KeyError):
        repo.get("x")


def test_zero_division():
    x = 0
    return 1 / x


def test_dict_fail():
    got = {"a": 1, "b": 2, "c": 3}
    assert got == {"a": 1, "b": 2, "c": 4}


@pytest.mark.parametrize("n,exp", [(1, 1), (2, 4), (3, 10)])
def test_square(n, exp):
    assert n * n == exp


def test_long_output_fail():
    lines = "\n".join(f"line {i}" for i in range(40))
    assert lines == "nope", lines
