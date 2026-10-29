{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T

say :: String -> TransIO ()
say = liftIO . putStrLn

thr :: Exception e => e -> TransIO ()
thr = liftIO . throwIO

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 2000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' does not finish)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "B5b those inside localBack disappear on leaving" $ do
    (return ()) `onUndo` (say "outer")
    localBack () $ do
       (return ()) `onUndo` (say "inner")
       return ()
    undo

  run "B5c localBack and backtracking with abduce inside" $ do
    (return ()) `onUndo` (say "outer")
    localBack () $ do
       (return ()) `onUndo` (say "inner")
       abduce
       undo
